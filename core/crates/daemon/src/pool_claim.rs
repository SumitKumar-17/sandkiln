//! Resolving and executing a `POST /sandboxes` request against a
//! configured pre-warmed pool — split out of `routes_sandbox.rs` once
//! this grew past a few hundred lines bolted onto sandbox-lifecycle
//! handlers, for the same reason `pool_replenisher.rs` is its own file
//! rather than folded into `pool.rs`: this is a distinct enough concern
//! (claiming, not configuring or replenishing) even though nothing here
//! has its own HTTP route of its own — `routes_sandbox::create_sandbox_core`
//! is still the only caller. See `crate::pool`'s module doc comment for
//! the feature's overall shape and scope.

use crate::error::AppError;
use crate::routes_sandbox::CreateSandboxRequest;
use crate::routes_snapshot::resume_snapshot_by_id;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

/// How long a `POST /sandboxes` request queues against a matching,
/// `max_count`-bounded pool that's at capacity with nothing warm ready,
/// before giving up with a real `503` — see `resolve_pool_claim`.
pub(crate) const POOL_QUEUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// How many times a failed warm claim retries against the same pool
/// before giving up and falling all the way through to an unattributed
/// cold create — see the retry loop's own comment in `create_sandbox_core`
/// for why this exists at all (keeping `max_count` honest under a high
/// resume failure rate) and why it's bounded rather than unbounded. A
/// genuinely pathological run of `MAX_POOL_CLAIM_ATTEMPTS` consecutive
/// bad warm snapshots still falls through unattributed at the end — an
/// accepted, rare edge case, not a guarantee this loop fully closes.
pub(crate) const MAX_POOL_CLAIM_ATTEMPTS: u32 = 3;

/// How many times `claim_from_pool` retries `Vm::update_metadata` right
/// after a resume, and how long it waits between attempts — see that
/// call site's own doc comment for the settling-window race this covers.
/// ~3s total budget, roughly matching the guest-visible side of the same
/// race (`scripts/integration-tests/18-pool.sh`'s own MMDS check retries
/// for up to 8s) rather than an arbitrarily shorter one.
const UPDATE_METADATA_MAX_ATTEMPTS: u32 = 10;
const UPDATE_METADATA_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// What a `POST /sandboxes` request resolves to against `AppState::pools`
/// — see `resolve_pool_claim`.
pub(crate) enum PoolClaim {
    /// A warm snapshot was ready and reserved (`Pool::take_warm` +
    /// `Pool::record_claim`, atomically under the same lock acquisition)
    /// — the caller must resume it and either commit the reservation
    /// (`PoolClaimGuard::commit`) or let it drop to release the slot.
    Warm { pool_id: String, snapshot_id: String },
    /// No pool configured for this key.
    NoPool,
    /// A pool matched, nothing was warm, but there was room under
    /// `max_count` to cold-create a new instance and reserve it — same
    /// commit-or-release contract as `Warm`.
    ColdSlot { pool_id: String },
}

/// Resolves a `POST /sandboxes` request against every configured pool,
/// waiting (up to `POOL_QUEUE_TIMEOUT`) if a matching pool exists but is
/// at `max_count` capacity with nothing warm — the "queueing" half of the
/// design in `crate::pool`'s module doc comment. Never holds
/// `AppState::pools`'s lock across an await point: each iteration locks,
/// makes a decision (or clones the pool's `Notify` to wait on), unlocks,
/// then optionally awaits outside the lock before looping back to
/// re-check with fresh state.
pub(crate) async fn resolve_pool_claim(state: &Arc<AppState>, key: &crate::pool::PoolKey) -> Result<PoolClaim, AppError> {
    let deadline = Instant::now() + POOL_QUEUE_TIMEOUT;
    loop {
        let notify = {
            let mut pools = state.pools.lock().unwrap();
            let Some(pool) = pools.values_mut().find(|p| p.config.key() == *key) else {
                return Ok(PoolClaim::NoPool);
            };
            if let Some(snapshot_id) = pool.take_warm() {
                pool.record_claim();
                return Ok(PoolClaim::Warm { pool_id: pool.config.id.clone(), snapshot_id });
            }
            if pool.has_room_for_new_claim() {
                pool.record_claim();
                return Ok(PoolClaim::ColdSlot { pool_id: pool.config.id.clone() });
            }
            pool.notify.clone()
        };

        let now = Instant::now();
        if now >= deadline {
            return Err(AppError::ServiceUnavailable(
                "matching pool is at its configured max_count and no capacity freed up in time — try again shortly".to_string(),
            ));
        }
        // Waiting past the deadline just means the next loop iteration's
        // own check finds it's out of time and returns the error above —
        // a `notify_waiters` that races with the timeout isn't lost, it's
        // simply re-checked as "was there room after all" first.
        let _ = tokio::time::timeout(deadline - now, notify.notified()).await;
    }
}

/// RAII handle for a slot `resolve_pool_claim` reserved
/// (`Pool::record_claim`) — releases it (`Pool::record_release`) on drop
/// unless `commit()` was called first. Call `commit()` exactly when a
/// real, live `Sandbox` now exists and durably owns this slot for the
/// rest of its life (released later by `routes_sandbox::destroy_sandbox_by_id`/
/// `routes_snapshot::snapshot_and_stop` instead) — every other exit path
/// (a failed resume, a failed health check, a failed cold boot) should
/// let this guard drop unclaimed so the slot goes back to the pool.
/// `pub(crate)`: also constructed directly by
/// `routes_sandbox::create_sandbox_cold` for the `ColdSlot` (and
/// unattributed) cold-boot path, not just `claim_from_pool` below.
pub(crate) struct PoolClaimGuard {
    state: Arc<AppState>,
    pool_id: Option<String>,
    committed: bool,
}

impl PoolClaimGuard {
    pub(crate) fn new(state: Arc<AppState>, pool_id: Option<String>) -> Self {
        Self { state, pool_id, committed: false }
    }

    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for PoolClaimGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        if let Some(pool_id) = &self.pool_id {
            if let Some(pool) = self.state.pools.lock().unwrap().get_mut(pool_id) {
                pool.record_release();
            }
        }
    }
}

/// Resumes a pool's warm snapshot and hands back a real, verified,
/// correctly-identified live sandbox — the "claim" half of pre-warmed
/// pools, called by `routes_sandbox::create_sandbox_core` once
/// `resolve_pool_claim` finds a `PoolClaim::Warm`.
///
/// Runs a real post-resume health check (`exec true`) before trusting the
/// resumed guest at all: resuming a snapshot has a real, non-rare failure
/// mode found live while building this — a resumed guest kernel can
/// panic early in boot (an early-boot divide-by-zero trap, confirmed via
/// Firecracker's own captured console log), and the whole Firecracker
/// process exits shortly after. Measured directly across clean, isolated
/// resumes across repeated clean, isolated test runs on this dev box —
/// this project's own `08-snapshots.sh` integration check still passes
/// every run because one resume isn't enough attempts to reliably hit
/// it, but a pool's whole reason to exist is resuming far more often
/// than a manual test ever would, which is exactly what surfaced a
/// failure rate this significant. Handing a caller a sandbox id that's
/// actually a corpse would be worse than never having a pool at all, so
/// this treats a failed health check as "the warm snapshot was bad" and
/// cleans up + falls back to a normal cold create (see this function's
/// caller) rather than either failing the request outright or returning
/// a broken id — see `destroy_unhealthy_claim`'s use of `Vm::force_stop`
/// for why the fallback itself doesn't pay a second unnecessary timeout.
///
/// The warm snapshot's own tags/name/MMDS content reflect
/// `pool_replenisher`'s placeholder request, not this one, and would
/// otherwise leak through as a confusing stale identity once a sandbox
/// does pass its health check — the guest-visible half of that (MMDS)
/// needs an explicit live Firecracker API call to fix, not just editing
/// the daemon's own `Sandbox` record; see
/// `sandkiln_vmm::vm::Vm::update_metadata`'s doc comment.
pub(crate) async fn claim_from_pool(
    state: &Arc<AppState>,
    snapshot_id: String,
    pool_id: String,
    request: &CreateSandboxRequest,
    egress: Option<sandkiln_vmm::egress::EgressPolicy>,
) -> Result<String, AppError> {
    // Retains history uniformly, same as any other resume -- the pool's
    // own warm-boot checkpoint isn't especially interesting to restore
    // later, but the caller's own *future* snapshots of this claimed
    // sandbox absolutely are, and there's no way to tell those two cases
    // apart from here. See `crate::snapshot_history`'s module doc comment.
    let id = resume_snapshot_by_id(state.clone(), snapshot_id, true).await?;

    let health_check = spawn_blocking_in_current_span("pool claim health check task panicked", {
        let state = state.clone();
        let id = id.clone();
        move || {
            let sandboxes = state.sandboxes.lock().unwrap();
            let sandbox = sandboxes.get(&id).expect("resume_snapshot_by_id above just inserted this id");
            sandbox.vm.call(&sandkiln_protocol::Request::Exec { command: "true".to_string(), args: vec![] })
        }
    })
    .await;

    if let Err(e) = health_check {
        tracing::warn!(sandbox_id = %id, error = %e, "pool-claimed sandbox failed its post-resume health check — tearing it down");
        destroy_unhealthy_claim(state, id).await;
        return Err(AppError::from(std::io::Error::other("pool-claimed sandbox failed its post-resume health check")));
    }

    let created_at = SystemTime::now();
    let metadata = serde_json::json!({ "id": id, "name": &request.name, "tags": &request.tags });
    let egress_target = {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).expect("resume_snapshot_by_id above just inserted this id");
        sandbox.network.as_ref().map(|network| (network.config.guest_ip, network.config.tap_device.clone()))
    };
    // Unlike MMDS staleness below, a requested egress policy failing to
    // apply is treated as fatal, not a warning — a caller asking for
    // network restrictions and silently getting an unrestricted sandbox
    // instead is a real security gap, not a cosmetic one. Applied outside
    // `state.sandboxes`'s lock (a real, potentially-slow `iptables`
    // shell-out, unlike the quick in-memory field writes below).
    if let (Some(policy), Some((guest_ip, tap_device))) = (&egress, &egress_target) {
        if let Err(e) = sandkiln_vmm::egress::apply(*guest_ip, tap_device, state.network.uplink(), policy) {
            destroy_unhealthy_claim(state, id).await;
            return Err(AppError::from(e));
        }
    }
    // A `/snapshot/load` resume (`resume_vm: true`) starts the guest as
    // part of that one API call, but calling `update_metadata`
    // immediately afterward can spuriously hit Firecracker's own
    // "operation not supported after starting the microVM" on
    // `/mmds/config` even though the resume itself already fully
    // succeeded — an existing, already-intermittent race under load on
    // this dev box (confirmed by running the same check repeatedly
    // against an *unmodified* daemon: it failed on one run out of three,
    // with the identical error, well before any of this session's other
    // changes), not something introduced by any specific feature.
    // Retried here with a brief pause between attempts — the same
    // "settling window" reasoning `scripts/integration-tests/18-pool.sh`'s
    // own MMDS-guest-visibility check already applies on the guest side —
    // rather than treated as a one-shot, best-effort call.
    let update_metadata_result = spawn_blocking_in_current_span("MMDS metadata refresh task panicked", {
        let state = state.clone();
        let id = id.clone();
        let metadata = metadata.clone();
        move || {
            let mut last_err = None;
            for attempt in 1..=UPDATE_METADATA_MAX_ATTEMPTS {
                let result = {
                    let sandboxes = state.sandboxes.lock().unwrap();
                    let sandbox = sandboxes.get(&id).expect("resume_snapshot_by_id above just inserted this id");
                    sandbox.vm.update_metadata(&metadata)
                };
                match result {
                    Ok(()) => return Ok(()),
                    Err(e) => {
                        last_err = Some(e);
                        if attempt < UPDATE_METADATA_MAX_ATTEMPTS {
                            std::thread::sleep(UPDATE_METADATA_RETRY_DELAY);
                        }
                    }
                }
            }
            Err(last_err.expect("loop runs at least once, so this is always populated by then"))
        }
    })
    .await;

    {
        let mut sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get_mut(&id).expect("resume_snapshot_by_id above just inserted this id");
        // Non-fatal even after exhausting retries: the sandbox already
        // passed its health check above and is genuinely usable either
        // way, just with stale MMDS content until something else resumes
        // or re-patches it (nothing does today) — worth a loud warning,
        // not a failed create over a metadata-only mismatch.
        if let Err(e) = update_metadata_result {
            tracing::warn!(sandbox_id = %id, error = %e, "failed to refresh MMDS metadata after resuming a pool-claimed sandbox, even after retrying");
        }
        sandbox.tags = request.tags.clone();
        sandbox.name = request.name.clone();
        sandbox.created_at = created_at;
        sandbox.egress = egress;
        // The slot `resolve_pool_claim` reserved for this claim is now
        // durably owned by this live `Sandbox` — released later by
        // `destroy_sandbox_by_id`/`snapshot_and_stop`, matching how
        // `create_sandbox_cold` commits its own `PoolClaimGuard`.
        sandbox.source_pool_id = Some(pool_id);
    }

    if let Err(e) = state.history.record_created(&id, request.name.as_deref(), &request.tags, request.image_id.as_deref(), created_at) {
        tracing::warn!(error = %e, sandbox_id = %id, "failed to record sandbox creation in history store");
    }
    state.metrics.record_sandbox_created();
    tracing::info!(sandbox_id = %id, "claimed a pre-warmed pool instance instead of cold-booting");
    Ok(id)
}

/// Tears down a pool-claimed sandbox that failed its post-resume health
/// check — same resource cleanup `routes_sandbox::destroy_sandbox_by_id`'s
/// destroy path does (release the network lease, remove the rootfs copy,
/// stop the `Vm`), just entered from a different failure mode: this
/// sandbox never got the chance to be a real, usable create in the first
/// place.
async fn destroy_unhealthy_claim(state: &Arc<AppState>, id: String) {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id);
    let Some(sandbox) = sandbox else { return };
    spawn_blocking_in_current_span("unhealthy claim teardown task panicked", {
        let state = state.clone();
        move || {
            if let Some(lease) = sandbox.network {
                let _ = state.network.release(lease);
            }
            let _ = std::fs::remove_file(&sandbox.rootfs_path);
            // `force_stop`, not `stop` -- the health check that got us
            // here already proved this VM isn't listening, so the
            // optimistic "sync before kill" `stop()` would otherwise try
            // has nothing to protect and would just burn its own
            // separate ~5-second retry budget for no reason.
            if let Err(e) = sandbox.vm.force_stop() {
                tracing::warn!(sandbox_id = %id, error = %e, "failed to fully stop an unhealthy pool-claimed sandbox's VM");
            }
        }
    })
    .await;
}

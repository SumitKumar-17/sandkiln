//! Resolving and executing a `POST /sandboxes` request against a
//! configured pre-warmed pool — split out of `routes_sandbox.rs` (claiming
//! is a distinct concern from configuring/replenishing, same reasoning as
//! `pool_replenisher.rs`'s own split, even with no HTTP route of its own:
//! `routes_sandbox::create_sandbox_core` is the only caller). See
//! `crate::pool` for the feature's overall shape.

use crate::error::AppError;
use crate::routes_sandbox::CreateSandboxRequest;
use crate::routes_snapshot::resume_snapshot_by_id;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

/// How long a `POST /sandboxes` request queues against a matching,
/// `max_count`-bounded pool at capacity with nothing warm, before a real
/// `503` — see `resolve_pool_claim`.
pub(crate) const POOL_QUEUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Retries of a failed warm claim against the same pool before falling
/// through to an unattributed cold create — bounded so `max_count` stays
/// honest under a high resume-failure rate without looping forever; a
/// pathological run of all 3 failing still falls through unattributed,
/// an accepted rare edge case.
pub(crate) const MAX_POOL_CLAIM_ATTEMPTS: u32 = 3;

/// Retries of `Vm::update_metadata` right after a resume (see that call
/// site for the settling-window race) — ~3s total, matching the
/// guest-visible side of the same race
/// (`scripts/integration-tests/18-pool.sh`'s MMDS check retries 8s).
const UPDATE_METADATA_MAX_ATTEMPTS: u32 = 10;
const UPDATE_METADATA_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(300);

/// What a `POST /sandboxes` request resolves to against `AppState::pools`.
pub(crate) enum PoolClaim {
    /// Warm snapshot reserved (`Pool::take_warm` + `record_claim`,
    /// atomic under one lock) — caller must resume it and either
    /// `PoolClaimGuard::commit` or let it drop to release the slot.
    Warm { pool_id: String, snapshot_id: String },
    /// No pool configured for this key.
    NoPool,
    /// Matched, nothing warm, but room under `max_count` to cold-create —
    /// same commit-or-release contract as `Warm`.
    ColdSlot { pool_id: String },
}

/// Resolves against every configured pool, waiting (up to
/// `POOL_QUEUE_TIMEOUT`) if a matching pool is at `max_count` with
/// nothing warm — `crate::pool`'s queueing half. Never holds
/// `AppState::pools`'s lock across an await: lock, decide (or clone the
/// pool's `Notify`), unlock, optionally await, loop with fresh state.
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
        // A `notify_waiters` racing the timeout isn't lost: the next loop
        // iteration just re-checks "was there room after all" first.
        let _ = tokio::time::timeout(deadline - now, notify.notified()).await;
    }
}

/// RAII handle for a slot `resolve_pool_claim` reserved — releases it
/// (`Pool::record_release`) on drop unless `commit()` ran first. Commit
/// only once a real live `Sandbox` durably owns the slot (released later
/// by `destroy_sandbox_by_id`/`snapshot_and_stop`); every other exit
/// (failed resume/health-check/cold-boot) drops unclaimed. `pub(crate)`:
/// also built directly by `create_sandbox_cold`'s `ColdSlot` path, not
/// just `claim_from_pool` below.
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

/// Resumes a pool's warm snapshot and hands back a verified,
/// correctly-identified live sandbox — the "claim" half of pre-warmed
/// pools, called once `resolve_pool_claim` finds `PoolClaim::Warm`.
///
/// Runs a real post-resume health check (`exec true`) first: resuming has
/// a real, non-rare failure mode found live building this feature — a
/// resumed guest kernel can panic early in boot (divide-by-zero trap,
/// confirmed via Firecracker's console log), killing the process shortly
/// after. Measured at a **1-in-3 to 2-in-3** failure rate across clean
/// isolated resumes on this dev box; `08-snapshots.sh`'s own integration
/// check never caught it because one resume isn't enough attempts to
/// reliably hit it. A failed check is treated as "the warm snapshot was
/// bad": clean up and fall through to a normal cold create (see caller),
/// never hand back a corpse id. `destroy_unhealthy_claim` uses
/// `Vm::force_stop` so this fallback doesn't pay a second timeout.
///
/// A warm snapshot's tags/name/MMDS reflect `pool_replenisher`'s
/// placeholder request, not this caller's — MMDS specifically needs a
/// live Firecracker API call to fix, not just a `Sandbox`-record edit
/// (`Vm::update_metadata`).
pub(crate) async fn claim_from_pool(
    state: &Arc<AppState>,
    snapshot_id: String,
    pool_id: String,
    request: &CreateSandboxRequest,
    egress: Option<sandkiln_vmm::egress::EgressPolicy>,
) -> Result<String, AppError> {
    // Retains history uniformly like any other resume -- no way to tell
    // "the pool's own warm checkpoint" from "the caller's future
    // snapshots of this sandbox" apart from here, so both get kept.
    let id = resume_snapshot_by_id(state.clone(), snapshot_id, true).await?;

    let health_check = spawn_blocking_in_current_span("pool claim health check task panicked", {
        let state = state.clone();
        let id = id.clone();
        move || {
            let sandboxes = state.sandboxes.lock().unwrap();
            let sandbox = sandboxes.get(&id).expect("resume_snapshot_by_id above just inserted this id");
            sandbox.vm.call(&sandkiln_protocol::Request::Exec {
                command: "true".to_string(),
                args: vec![],
                env: std::collections::HashMap::new(),
            })
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
    // Fatal, not a warning, unlike MMDS staleness below -- silently
    // handing back an unrestricted sandbox when the caller asked for
    // egress restrictions is a real security gap. Outside the sandboxes
    // lock: a real, potentially-slow iptables shell-out.
    if let (Some(policy), Some((guest_ip, tap_device))) = (&egress, &egress_target) {
        if let Err(e) = sandkiln_vmm::egress::apply(*guest_ip, tap_device, state.network.uplink(), policy) {
            destroy_unhealthy_claim(state, id).await;
            return Err(AppError::from(e));
        }
    }
    // `update_metadata` right after a `resume_vm: true` resume can
    // spuriously hit Firecracker's "operation not supported after
    // starting the microVM" on /mmds/config even though the resume fully
    // succeeded -- a pre-existing, load-related race (reproduces on an
    // unmodified daemon, ~1 run in 3), not feature-specific. Retried with
    // a pause between attempts rather than treated as one-shot.
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
        // Non-fatal even exhausted: already passed its health check, just
        // stale MMDS until something re-patches it (nothing does today).
        if let Err(e) = update_metadata_result {
            tracing::warn!(sandbox_id = %id, error = %e, "failed to refresh MMDS metadata after resuming a pool-claimed sandbox, even after retrying");
        }
        sandbox.tags = request.tags.clone();
        sandbox.name = request.name.clone();
        sandbox.created_at = created_at;
        sandbox.egress = egress;
        sandbox.env = request.env.clone();
        // Slot now durably owned by this live Sandbox -- released later
        // by destroy_sandbox_by_id/snapshot_and_stop.
        sandbox.source_pool_id = Some(pool_id);
    }

    if let Err(e) = state.history.record_created(&id, request.name.as_deref(), &request.tags, request.image_id.as_deref(), created_at) {
        tracing::warn!(error = %e, sandbox_id = %id, "failed to record sandbox creation in history store");
    }
    state.metrics.record_sandbox_created();
    tracing::info!(sandbox_id = %id, "claimed a pre-warmed pool instance instead of cold-booting");
    Ok(id)
}

/// Tears down a pool-claimed sandbox that failed its health check — same
/// cleanup as `destroy_sandbox_by_id`'s destroy path (lease, rootfs, VM),
/// just from a sandbox that never got to be a usable create.
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
            // force_stop, not stop -- the health check already proved
            // this VM isn't listening, so stop()'s "sync before kill"
            // has nothing to protect and would burn its own ~5s budget.
            if let Err(e) = sandbox.vm.force_stop() {
                tracing::warn!(sandbox_id = %id, error = %e, "failed to fully stop an unhealthy pool-claimed sandbox's VM");
            }
        }
    })
    .await;
}

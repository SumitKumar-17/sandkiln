//! Sandbox lifecycle: create, list, stop. Exec and file operations live in
//! `routes_exec` — split out because they share a `call_agent` helper that
//! has nothing to do with lifecycle management. Name-based lookup/
//! get-or-create lives in `routes_sandbox_name` — a distinct enough
//! concern (crosses into snapshot territory, needs the per-name lock)
//! that folding it in here would blow well past this file's existing
//! ~300-line-ish shape for no structural reason.

use crate::error::AppError;
use crate::routes_drives::DriveAttachment;
use crate::routes_snapshot::{resume_snapshot_by_id, snapshot_and_stop, SnapshotBlocked, SnapshotStopError};
use crate::sandbox::Sandbox;
use crate::state::{can_attach_read_only, describe_drive_holders, AppState, AttachedDrive};
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use sandkiln_vmm::jailer::JailLaunch;
use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::{DriveConfig, RateLimiter, TokenBucket, Vm, VmConfig};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Deserialize, Default)]
pub struct CreateSandboxRequest {
    /// Caller-given identity, unique among live sandboxes and held
    /// snapshots at the moment it's claimed (`409` if already taken).
    /// Optional — naming is opt-in. See `Sandbox::name`'s doc comment and
    /// `routes_sandbox_name` for looking a sandbox up by name later.
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) tags: HashMap<String, String>,
    /// Existing persistent drives (see `POST /drives`) to attach at boot,
    /// each becoming its own block device inside the guest.
    #[serde(default)]
    pub(crate) drives: Vec<DriveAttachment>,
    /// Overrides the daemon's configured default vCPU count
    /// (`SANDKILN_VCPU_COUNT`) for this one sandbox. Omitted means "use
    /// the default" — today's behavior, unchanged. Rejected outright
    /// (`400`) rather than clamped if it's `0` or exceeds the configured
    /// ceiling (`SANDKILN_MAX_VCPU_COUNT`) — see `resolve_resource_override`.
    #[serde(default)]
    pub(crate) vcpu_count: Option<u8>,
    /// Overrides the daemon's configured default memory size in MiB
    /// (`SANDKILN_MEM_SIZE_MIB`) for this one sandbox. Same semantics as
    /// `vcpu_count` above, checked against `SANDKILN_MAX_MEM_SIZE_MIB`.
    #[serde(default)]
    pub(crate) mem_size_mib: Option<u32>,
    /// Boots from a registered image (see `POST /images`,
    /// `sandkiln_vmm::image::ImageStore`) instead of the daemon's
    /// `SANDKILN_BASE_ROOTFS` default. Omitted means "use the default" —
    /// today's behavior, unchanged. Rejected with `404` if no image with
    /// this id is currently registered.
    #[serde(default)]
    pub(crate) image_id: Option<String>,
    /// Caps host I/O for this sandbox — Firecracker's own token-bucket
    /// rate limiter, applied to the rootfs drive, every attached drive,
    /// and the network interface (both directions). Omitted means
    /// unlimited host I/O — today's behavior, unchanged. At least one of
    /// `bandwidth_bytes_per_sec`/`ops_per_sec` must be set and non-zero if
    /// this is present at all — `400` otherwise, same "reject, don't
    /// silently no-op" convention as `vcpu_count`/`mem_size_mib`.
    #[serde(default)]
    pub(crate) rate_limit: Option<RateLimitRequest>,
    /// Outbound network policy for this sandbox — see
    /// `sandkiln_vmm::egress`'s module doc comment for the full design.
    /// Omitted means today's behavior, unchanged: unrestricted outbound
    /// through the shared bridge's existing catch-all rule. Unlike
    /// `drives`/`rate_limit`, this can still match a pre-warmed pool (see
    /// `crate::pool`) — egress is enforced via host-side iptables rules
    /// applied *after* boot/resume, never baked into Firecracker's own
    /// VM/snapshot state, so it's compatible with any warm snapshot
    /// regardless of what policy (if any) the pool's own replenishment
    /// boot used.
    #[serde(default)]
    pub(crate) egress: Option<EgressPolicyRequest>,
}

#[derive(Deserialize, Clone, Copy)]
pub struct RateLimitRequest {
    #[serde(default)]
    pub(crate) bandwidth_bytes_per_sec: Option<u64>,
    #[serde(default)]
    pub(crate) ops_per_sec: Option<u64>,
}

#[derive(Deserialize, Clone)]
pub struct EgressPolicyRequest {
    pub(crate) mode: EgressModeRequest,
    /// IPv4 CIDRs (`"10.0.0.0/8"`) — validated in `resolve_egress_policy`,
    /// not here, so a malformed one gets one clear `400` covering every
    /// entry in both lists rather than whichever `serde` error format a
    /// custom `Deserialize` impl would produce.
    #[serde(default)]
    pub(crate) allow_cidrs: Vec<String>,
    #[serde(default)]
    pub(crate) deny_cidrs: Vec<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum EgressModeRequest {
    AllowAll,
    DenyAll,
}

#[derive(Serialize)]
pub struct CreateSandboxResponse {
    id: String,
}

/// RAII pairing for `AppState::reserve_pending_image_boot`/
/// `release_pending_image_boot` — releases the claim on drop, covering
/// every exit path out of `create_sandbox` (the happy path, an early `?`
/// return, or a panic unwind out of the boot task) rather than requiring
/// every one of those to remember to release it manually. A no-op `Drop`
/// when `image_id` is `None` (the common case — most boots don't reference
/// a registered image at all).
struct PendingImageBootGuard {
    state: Arc<AppState>,
    image_id: Option<String>,
}

impl Drop for PendingImageBootGuard {
    fn drop(&mut self) {
        if let Some(image_id) = &self.image_id {
            self.state.release_pending_image_boot(image_id);
        }
    }
}

#[tracing::instrument(skip(state, body))]
pub async fn create_sandbox(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<CreateSandboxResponse>, AppError> {
    let request: CreateSandboxRequest = if body.is_empty() {
        CreateSandboxRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?
    };

    // Held for the rest of this function whenever a name was given —
    // serializes this call against any other concurrent claim of the
    // same name (another named `create_sandbox`, or
    // `routes_sandbox_name::get_or_create_sandbox`) so two callers racing
    // on a brand-new name can't both pass the uniqueness check below and
    // both create a sandbox. See `AppState::lock_name`.
    let _name_guard = match &request.name {
        Some(name) => {
            crate::routes_sandbox_name::validate_name(name).map_err(AppError::BadRequest)?;
            let guard = state.lock_name(name).await;
            if let Some(holder) = state.name_holder(name) {
                return Err(AppError::Conflict(format!("name '{name}' is already used by {holder}")));
            }
            Some(guard)
        }
        None => None,
    };

    let id = create_sandbox_core(&state, request).await?;
    Ok(Json(CreateSandboxResponse { id }))
}

/// The actual "boot a sandbox" mechanics, shared by `create_sandbox`
/// (`POST /sandboxes`) and `routes_sandbox_name::get_or_create_sandbox`'s
/// create-fresh path. Does **not** check name uniqueness itself — both
/// callers already did that under `AppState::lock_name` before reaching
/// here, and re-checking would just be redundant work under the same
/// lock they're still holding.
/// How long a `POST /sandboxes` request matching a `max_count`-bounded
/// pool at capacity waits for room before giving up (`503`, see
/// `AppError::ServiceUnavailable`) rather than either exceeding the
/// ceiling or hanging the caller's request forever. Not currently
/// configurable — see `crate::pool`'s "scoped honestly" notes; a fixed
/// default is enough for a first cut of queueing.
pub(crate) const POOL_QUEUE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) async fn create_sandbox_core(state: &Arc<AppState>, request: CreateSandboxRequest) -> Result<String, AppError> {
    if let Some(dup) = first_duplicate(request.drives.iter().map(|d| d.id.as_str())) {
        return Err(AppError::BadRequest(format!("drive listed more than once: {dup}")));
    }
    for drive in &request.drives {
        if !state.drives.exists(&drive.id) {
            return Err(AppError::DriveNotFound(drive.id.clone()));
        }
    }
    for drive in &request.drives {
        let holders = state.drive_holders(&drive.id);
        let existing_read_only: Vec<bool> = holders.iter().map(|h| h.read_only).collect();
        if !can_attach_read_only(&existing_read_only, drive.read_only) {
            return Err(AppError::Conflict(format!(
                "drive {} is already attached to {} — a read-write attachment needs exclusive access; \
                 only simultaneous read-only attachments are allowed",
                drive.id,
                describe_drive_holders(&holders)
            )));
        }
    }

    let vcpu_count = resolve_resource_override(request.vcpu_count, state.config.vcpu_count, state.config.max_vcpu_count, "vcpu_count")
        .map_err(AppError::BadRequest)?;
    let mem_size_mib =
        resolve_resource_override(request.mem_size_mib, state.config.mem_size_mib, state.config.max_mem_size_mib, "mem_size_mib")
            .map_err(AppError::BadRequest)?;
    let rate_limit = resolve_rate_limit(&request.rate_limit).map_err(AppError::BadRequest)?;
    let egress = resolve_egress_policy(&request.egress).map_err(AppError::BadRequest)?;

    // A matching, ready pre-warmed pool (see `crate::pool`) lets this
    // resume a warm snapshot instead of paying full cold-create cost —
    // only when the request doesn't need anything a warm snapshot can't
    // already provide. Drives and a custom rate limit are both baked into
    // a VM's state at boot time and a warm snapshot was booted with
    // neither, so either one present here means this request can never
    // match a pool, regardless of image/resources — falls through to the
    // normal cold-create path below instead of silently ignoring them (and
    // never queues on a `max_count`-bounded pool either, for the same
    // reason: it was never going to match that pool anyway).
    if request.drives.is_empty() && request.rate_limit.is_none() {
        let key = crate::pool::PoolKey { image_id: request.image_id.clone(), vcpu_count, mem_size_mib };
        // Bounded retry, not a single attempt: a warm claim failing its
        // post-resume health check (see `claim_from_pool`'s doc comment —
        // measured at up to ~2-in-3 resumes, not a rare corner case)
        // releases its reserved slot right back to the pool via
        // `PoolClaimGuard`'s drop — re-resolving immediately afterward is
        // what lets the resulting fallback cold-create still count
        // against `max_count` instead of silently bypassing it. Without
        // this loop, a `max_count`-bounded pool with a high resume
        // failure rate could end up running noticeably more live
        // instances than its own configured ceiling. Bounded (not
        // unbounded) purely as a safety margin against a pathological
        // run of consecutive bad warm snapshots — see
        // `MAX_POOL_CLAIM_ATTEMPTS`.
        for _ in 0..MAX_POOL_CLAIM_ATTEMPTS {
            match resolve_pool_claim(state, &key).await? {
                PoolClaim::Warm { pool_id, snapshot_id } => {
                    let guard = PoolClaimGuard::new(state.clone(), Some(pool_id.clone()));
                    match claim_from_pool(state, snapshot_id, pool_id, &request, egress.clone()).await {
                        Ok(id) => {
                            guard.commit();
                            return Ok(id);
                        }
                        Err(e) => {
                            // `guard` drops at the end of this arm (not
                            // committed), releasing the slot this claim
                            // reserved back to the pool — the next loop
                            // iteration's `resolve_pool_claim` sees that
                            // freed room immediately.
                            tracing::warn!(error = %e, "pool claim failed — retrying against the pool once more before falling back unattributed");
                        }
                    }
                }
                PoolClaim::ColdSlot { pool_id } => {
                    // Room under `max_count`, nothing warm ready right
                    // now — proceed into the cold-create path below,
                    // attributed to this pool so the reserved slot is
                    // either committed to the resulting live `Sandbox` or
                    // released on any failure (`PoolClaimGuard`,
                    // constructed inside).
                    return create_sandbox_cold(state, request, Some(pool_id), vcpu_count, mem_size_mib, rate_limit, egress).await;
                }
                PoolClaim::NoPool => break, // no pool configured for this profile at all — today's original, totally unattributed behavior.
            }
        }
    }

    create_sandbox_cold(state, request, None, vcpu_count, mem_size_mib, rate_limit, egress).await
}

/// How many times a failed warm claim retries against the same pool
/// before giving up and falling all the way through to an unattributed
/// cold create — see the retry loop's own comment in `create_sandbox_core`
/// for why this exists at all (keeping `max_count` honest under a high
/// resume failure rate) and why it's bounded rather than unbounded. A
/// genuinely pathological run of `MAX_POOL_CLAIM_ATTEMPTS` consecutive
/// bad warm snapshots still falls through unattributed at the end — an
/// accepted, rare edge case, not a guarantee this loop fully closes.
const MAX_POOL_CLAIM_ATTEMPTS: u32 = 3;

/// What a `POST /sandboxes` request resolves to against `AppState::pools`
/// — see `resolve_pool_claim`.
enum PoolClaim {
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
async fn resolve_pool_claim(state: &Arc<AppState>, key: &crate::pool::PoolKey) -> Result<PoolClaim, AppError> {
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
/// rest of its life (released later by `destroy_sandbox_by_id`/
/// `routes_snapshot::snapshot_and_stop` instead) — every other exit path
/// (a failed resume, a failed health check, a failed cold boot) should
/// let this guard drop unclaimed so the slot goes back to the pool.
struct PoolClaimGuard {
    state: Arc<AppState>,
    pool_id: Option<String>,
    committed: bool,
}

impl PoolClaimGuard {
    fn new(state: Arc<AppState>, pool_id: Option<String>) -> Self {
        Self { state, pool_id, committed: false }
    }

    fn commit(mut self) {
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

/// The actual cold-boot mechanics — unchanged from before pools existed,
/// except for `pool_id`: `Some` when `create_sandbox_core` reserved a
/// slot for this create against a `max_count`-bounded pool (see
/// `PoolClaim::ColdSlot`), threaded through to `PoolClaimGuard` (released
/// on any failure below) and `Sandbox::source_pool_id` (committed,
/// permanently owning the slot, on success).
async fn create_sandbox_cold(
    state: &Arc<AppState>,
    request: CreateSandboxRequest,
    pool_id: Option<String>,
    vcpu_count: u8,
    mem_size_mib: u32,
    rate_limit: Option<RateLimiter>,
    egress: Option<sandkiln_vmm::egress::EgressPolicy>,
) -> Result<String, AppError> {
    let pool_guard = PoolClaimGuard::new(state.clone(), pool_id.clone());

    // Checked and reserved before the (slow) boot starts, not just relied
    // on implicitly once the sandbox is inserted into `state.sandboxes` at
    // the end — closes the window where a concurrent `DELETE /images/:id`
    // could otherwise remove the very file this boot's rootfs copy is
    // reading from. `_image_boot_guard` releases the reservation on every
    // exit path below (success, an early `?` return, or a panicking
    // `spawn_blocking` task), see `PendingImageBootGuard`.
    if let Some(image_id) = &request.image_id {
        if !state.images.exists(image_id) {
            return Err(AppError::ImageNotFound(image_id.clone()));
        }
        state.reserve_pending_image_boot(image_id);
    }
    let _image_boot_guard = PendingImageBootGuard { state: state.clone(), image_id: request.image_id.clone() };
    let base_rootfs_source = match &request.image_id {
        Some(image_id) => state.images.path_for(image_id),
        None => state.config.base_rootfs_path.clone(),
    };

    let id = Uuid::new_v4().to_string();
    // Guest-reachable via Firecracker's own MMDS (see VmConfig::metadata's
    // doc comment) -- built from what the caller already gave us, not new
    // input; borrowed rather than cloned since `request.tags`/`request.name`
    // are still needed below for the `Sandbox` this call ultimately builds.
    let metadata = serde_json::json!({ "id": id, "name": &request.name, "tags": &request.tags });
    let rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{id}.ext4"));
    let attached_drives: Vec<AttachedDrive> =
        request.drives.iter().map(|d| AttachedDrive { drive_id: d.id.clone(), read_only: d.read_only }).collect();
    // Firecracker's own drive_id namespace is per-VM, but prefix these
    // anyway to keep them unambiguously distinct from the reserved
    // "rootfs" id regardless of what a drive's storage id looks like.
    // Firecracker only allows alphanumerics and underscores in a
    // drive_id (drive ids here are UUIDs, which contain hyphens) — '-'
    // has to become '_', not just the prefix's own separator.
    let extra_drives: Vec<DriveConfig> = request
        .drives
        .iter()
        .map(|d| DriveConfig {
            drive_id: format!("drive_{}", d.id.replace('-', "_")),
            path_on_host: state.drives.path_for(&d.id),
            read_only: d.read_only,
        })
        .collect();

    let (vm, network, jail_id) = spawn_blocking_in_current_span("boot task panicked", {
        let state = state.clone();
        let rootfs_path = rootfs_path.clone();
        let base_rootfs_source = base_rootfs_source.clone();
        let egress = egress.clone();
        move || -> std::io::Result<(Vm, Lease, Option<u32>)> {
            let span = tracing::Span::current();
            // Copying the rootfs and leasing a network are independent —
            // running them concurrently overlaps the (currently dominant)
            // cost of the rootfs copy with the lease instead of paying for
            // both serially.
            let (copy_result, lease_result) = std::thread::scope(|scope| {
                let copy_handle = scope.spawn(|| span.in_scope(|| clone_rootfs(&base_rootfs_source, &rootfs_path)));
                let lease_handle = scope.spawn(|| span.in_scope(|| state.network.lease()));
                (copy_handle.join().expect("rootfs copy thread panicked"), lease_handle.join().expect("lease thread panicked"))
            });
            copy_result?;
            let lease = lease_result?;

            // A jail id (uid == gid, leased from `AppState::jailer_ids`)
            // is the third resource a sandbox needs, alongside the rootfs
            // copy and the network lease — leased after both since it's
            // an in-memory pool pop (no I/O to overlap with), and
            // released immediately if leasing it is the thing that fails,
            // exactly like a failed `Vm::boot` releases the network lease
            // below.
            let jail_id = match &state.jailer_ids {
                Some(pool) => match pool.lease() {
                    Ok(id) => Some(id),
                    Err(e) => {
                        let _ = state.network.release(lease);
                        return Err(e);
                    }
                },
                None => None,
            };
            let jail = jail_id.and_then(|id| {
                state.config.jailer.as_ref().map(|j| JailLaunch {
                    jailer_bin: j.jailer_bin.clone(),
                    chroot_base_dir: j.chroot_base_dir.clone(),
                    uid: id,
                    gid: id,
                })
            });

            let boot_started = Instant::now();
            let vm = Vm::boot(&VmConfig {
                firecracker_bin: state.config.firecracker_bin.clone(),
                kernel_path: state.config.kernel_path.clone(),
                rootfs_path,
                vcpu_count,
                mem_size_mib,
                network: Some(lease.config.clone()),
                extra_drives,
                jail,
                rate_limit,
                metadata: Some(metadata),
            });
            match vm {
                Ok(vm) => {
                    state.metrics.record_boot_duration_ms(boot_started.elapsed().as_secs_f64() * 1000.0);
                    // Applied after boot succeeds, before this sandbox is
                    // ever visible to a caller -- a requested policy that
                    // fails to apply must not silently leave the sandbox
                    // unrestricted, so this is treated exactly like a
                    // failed `Vm::boot`: tear everything down and return
                    // the error rather than let a broken-but-unenforced
                    // policy through.
                    if let Some(policy) = &egress {
                        if let Err(e) = sandkiln_vmm::egress::apply(lease.config.guest_ip, &lease.config.tap_device, state.network.uplink(), policy) {
                            let _ = vm.stop();
                            let _ = state.network.release(lease);
                            if let (Some(id), Some(pool)) = (jail_id, &state.jailer_ids) {
                                pool.release(id);
                            }
                            return Err(e);
                        }
                    }
                    Ok((vm, lease, jail_id))
                }
                Err(e) => {
                    let _ = state.network.release(lease);
                    if let (Some(id), Some(pool)) = (jail_id, &state.jailer_ids) {
                        pool.release(id);
                    }
                    Err(e)
                }
            }
        }
    })
    .await?;

    let created_at = SystemTime::now();
    // Best-effort: the sandbox has already actually booted by this point
    // (a real, running VM) — failing the whole request over a history-DB
    // write error would waste it for no benefit, so this only warns.
    if let Err(e) = state.history.record_created(&id, request.name.as_deref(), &request.tags, request.image_id.as_deref(), created_at) {
        tracing::warn!(error = %e, sandbox_id = %id, "failed to record sandbox creation in history store");
    }

    let sandbox = Sandbox {
        id: id.clone(),
        vm,
        network: Some(network),
        rootfs_path,
        attached_drives,
        image_id: request.image_id,
        jail_id,
        tags: request.tags,
        created_at,
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        source_snapshot_id: None,
        name: request.name,
        pty_session_count: Default::default(),
        source_pool_id: pool_id,
        egress,
        // A cold create -- fresh boot or a pool's own warm-replenishment
        // boot -- is always a root of its own lineage.
        parent_snapshot_id: None,
    };
    state.sandboxes.lock().unwrap().insert(id.clone(), sandbox);
    state.metrics.record_sandbox_created();
    // The slot `resolve_pool_claim` reserved (if any) is now durably
    // owned by the live `Sandbox` above via `source_pool_id` — released
    // later by `destroy_sandbox_by_id`/`snapshot_and_stop`, not this
    // guard, which would otherwise release it right back on drop here.
    pool_guard.commit();

    Ok(id)
}

/// Finishes a pre-warmed pool claim (see `crate::pool`): resumes the warm
/// snapshot `create_sandbox_core` already popped off a matching pool,
/// verifies the result is actually alive, then overwrites its identity
/// with what the *caller* actually asked for.
///
/// **The health check is load-bearing, not defensive theater — and not
/// a rare edge case either.** Found live while building this feature:
/// Firecracker's snapshot/restore has a real failure mode where the
/// restored guest kernel panics early in boot (confirmed via its
/// captured console log — an early-boot divide-by-zero trap in the
/// console driver, restored CPU/timer state interacting badly with
/// timing-sensitive init code) and the whole Firecracker process exits
/// shortly after. Measured directly at roughly **1-in-3 to 2-in-3**
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
async fn claim_from_pool(
    state: &Arc<AppState>,
    snapshot_id: String,
    pool_id: String,
    request: &CreateSandboxRequest,
    egress: Option<sandkiln_vmm::egress::EgressPolicy>,
) -> Result<String, AppError> {
    let id = resume_snapshot_by_id(state.clone(), snapshot_id).await?;

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
    {
        let mut sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get_mut(&id).expect("resume_snapshot_by_id above just inserted this id");
        // Non-fatal if this fails: the sandbox already passed its health
        // check above and is genuinely usable either way, just with
        // stale MMDS content until something else resumes or re-patches
        // it (nothing does today) — worth a loud warning, not a failed
        // create over a metadata-only mismatch.
        if let Err(e) = sandbox.vm.update_metadata(&metadata) {
            tracing::warn!(sandbox_id = %id, error = %e, "failed to refresh MMDS metadata after resuming a pool-claimed sandbox");
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
/// check — same resource cleanup `stop_sandbox_by_id`'s destroy path
/// does (release the network lease, remove the rootfs copy, stop the
/// `Vm`), just entered from a different failure mode: this sandbox never
/// got the chance to be a real, usable create in the first place.
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

#[derive(Serialize)]
pub struct SandboxSummary {
    id: String,
    created_at_unix: u64,
    tags: HashMap<String, String>,
    name: Option<String>,
}

#[derive(Serialize)]
pub struct ListSandboxesResponse {
    sandboxes: Vec<SandboxSummary>,
}

/// Filters by tag by passing `?tag.<key>=<value>` query params — a
/// sandbox must match every one given, if any are given.
pub async fn list_sandboxes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<ListSandboxesResponse> {
    let tag_filters: Vec<(&str, &str)> = query
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("tag.").map(|key| (key, v.as_str())))
        .collect();

    let sandboxes = state
        .sandboxes
        .lock()
        .unwrap()
        .values()
        .filter(|s| tag_filters.iter().all(|(k, v)| s.tags.get(*k).map(String::as_str) == Some(v)))
        .map(|s| SandboxSummary {
            id: s.id.clone(),
            created_at_unix: s.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            tags: s.tags.clone(),
            name: s.name.clone(),
        })
        .collect();
    Json(ListSandboxesResponse { sandboxes })
}

#[derive(Serialize)]
pub struct HistoryRecordBody {
    id: String,
    name: Option<String>,
    tags: HashMap<String, String>,
    image_id: Option<String>,
    created_at_unix: u64,
    ended_at_unix: Option<u64>,
    end_reason: Option<String>,
    final_snapshot_id: Option<String>,
}

impl From<sandkiln_store::HistoryRecord> for HistoryRecordBody {
    fn from(r: sandkiln_store::HistoryRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            tags: r.tags,
            image_id: r.image_id,
            created_at_unix: r.created_at_unix,
            ended_at_unix: r.ended_at_unix,
            end_reason: r.end_reason,
            final_snapshot_id: r.final_snapshot_id,
        }
    }
}

#[derive(Serialize)]
pub struct SandboxHistoryResponse {
    history: Vec<HistoryRecordBody>,
}

/// Durable sandbox lifecycle history, independent of the live
/// `sandboxes` map — survives a daemon restart, unlike `GET /sandboxes`.
/// See `sandkiln-store`'s module doc comment for exactly what this does
/// and doesn't mean (it cannot bring a stopped sandbox back to life).
/// `?live_only=true`/`?live_only=false` filters; `?limit=<n>` caps the
/// row count (defaults to `sandkiln_store::DEFAULT_LIST_LIMIT`). Always
/// newest-created first.
pub async fn sandbox_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<SandboxHistoryResponse>, AppError> {
    let live_only = query.get("live_only").map(|v| v == "true");
    let limit = query.get("limit").and_then(|v| v.parse::<u32>().ok());
    let filter = sandkiln_store::HistoryFilter { live_only, limit };

    let records = spawn_blocking_in_current_span("history list task panicked", {
        let state = state.clone();
        move || state.history.list(&filter)
    })
    .await
    .map_err(|e| AppError::Internal(std::io::Error::other(e.to_string())))?;

    Ok(Json(SandboxHistoryResponse { history: records.into_iter().map(HistoryRecordBody::from).collect() }))
}

/// Whether stopping preserves this sandbox's state (the default) or
/// destroys it outright — the query-string form of `DELETE
/// /sandboxes/:id?keep=false`. Pulled out as a pure parser for direct
/// unit testing, mirroring `resolve_resource_override` above.
fn parse_keep(params: &HashMap<String, String>) -> Result<bool, String> {
    match params.get("keep").map(String::as_str) {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(format!("invalid 'keep' query parameter '{other}': expected 'true' or 'false'")),
    }
}

#[derive(Serialize)]
pub struct StopSandboxResponse {
    /// Whether this stop actually produced a new `Snapshot` this sandbox
    /// can be resumed from. `false` either because the caller explicitly
    /// asked for full destruction (`?keep=false`) or because this
    /// particular sandbox had nothing new to preserve (a fork — see
    /// `stop_sandbox_by_id`'s doc comment).
    kept: bool,
    snapshot_id: Option<String>,
}

/// `DELETE /sandboxes/:id` — stops a sandbox. As of the "persistent by
/// default" behavior (see `stop_sandbox_by_id`'s doc comment), the
/// default response is `200` with a JSON body reporting what happened,
/// not the old bare `204`: there is now new information worth returning
/// (a snapshot id) that wasn't there when this only ever destroyed. The
/// explicit-destroy path (`?keep=false`) keeps the original `204`
/// contract exactly — nothing new to report, unchanged from before this
/// feature existed.
#[tracing::instrument(skip(state))]
pub async fn stop_sandbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<axum::response::Response, AppError> {
    let keep = parse_keep(&params).map_err(AppError::BadRequest)?;

    let outcome = stop_sandbox_by_id(state, id.clone(), keep).await.map_err(|e| match e {
        StopError::NotFound => AppError::NotFound(id),
        StopError::CannotPreserve(reason) => cannot_preserve_error(reason),
        StopError::Io(e) => AppError::from(e),
    })?;

    if !keep {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let (kept, snapshot_id) = match outcome {
        StopOutcome::Snapshotted(snapshot_id) => (true, Some(snapshot_id)),
        StopOutcome::Destroyed => (false, None),
    };
    Ok(Json(StopSandboxResponse { kept, snapshot_id }).into_response())
}

/// What `stop_sandbox_by_id` actually did — used to shape `DELETE`'s
/// response body; the idle reaper (`idle_reaper::run`) only cares whether
/// it succeeded at all.
pub(crate) enum StopOutcome {
    Snapshotted(String),
    Destroyed,
}

/// Every way `stop_sandbox_by_id` can fail. Distinct from `AppError` so
/// callers other than the HTTP route (namely `idle_reaper`) can react to
/// `CannotPreserve` without going through an HTTP-status-shaped type —
/// see `idle_reaper::reap_once`, which falls back to a full destroy on
/// exactly this variant instead of leaking the sandbox forever.
pub(crate) enum StopError {
    NotFound,
    /// `keep=true` was requested (explicitly or by default) but this
    /// particular sandbox structurally can't be snapshotted right now —
    /// see `SnapshotBlocked`. Note a *forked* sandbox never produces this:
    /// `stop_sandbox_by_id` treats that case as a silent, correct destroy
    /// rather than an error (see its doc comment), since a fork has
    /// nothing new to preserve. Only a jailed sandbox reaches here.
    CannotPreserve(SnapshotBlocked),
    Io(std::io::Error),
}

fn cannot_preserve_error(reason: SnapshotBlocked) -> AppError {
    match reason {
        SnapshotBlocked::Jailed => AppError::Conflict(
            "this sandbox is jailed, and snapshotting a jailed sandbox is not supported yet — it can't be \
             stopped-and-preserved by default; retry with ?keep=false to destroy it instead"
                .to_string(),
        ),
        // Unreachable via `stop_sandbox_by_id` today (forks are handled
        // as a silent destroy, not this error) — kept exhaustive rather
        // than `unreachable!()` so a future change to that logic fails to
        // compile loudly instead of panicking at runtime if it ever does
        // start reaching here.
        SnapshotBlocked::ForkedFrom(source) => AppError::Conflict(format!(
            "this sandbox was forked from snapshot {source} and can't be independently snapshotted — retry with \
             ?keep=false to destroy it instead"
        )),
    }
}

/// Stops a sandbox. `keep=true` (the default — both `DELETE
/// /sandboxes/:id` with no query param and `idle_reaper`'s automatic
/// stop) is the ROADMAP's "persistent by default" behavior: this
/// internally does what `POST /sandboxes/:id/snapshot` does (pause,
/// snapshot to disk, stop the VM), landing the sandbox as a `Snapshot`
/// record — including its `name`, if it had one — instead of deleting
/// its rootfs and releasing its network lease for good. `keep=false` is
/// the explicit opt-out, for a caller who genuinely wants full
/// destruction with nothing left behind (e.g. a short-lived CI sandbox
/// that will never come back) — it does exactly what stopping a sandbox
/// always used to do.
///
/// A forked sandbox (`source_snapshot_id.is_some()`) is a special case
/// under `keep=true`: it shares its rootfs file with the snapshot it came
/// from rather than owning a private copy, so it structurally can't be
/// snapshotted again on its own (see `SnapshotBlocked::ForkedFrom`) — but
/// that's fine, not an error, because that shared snapshot *already is*
/// this identity's durable state, untouched by the fork's ephemeral VM.
/// There's nothing new to preserve, so `keep=true`'s intent is already
/// satisfied by destroying just the fork (which, per
/// `destroy_sandbox_by_id`'s own doc comment, never touches a fork's
/// shared rootfs/network anyway). A jailed sandbox has no such fallback —
/// Firecracker's jailed snapshot/resume path genuinely isn't supported —
/// so that case surfaces as `StopError::CannotPreserve` instead of
/// silently destroying state a caller's default expectation says should
/// have survived.
///
/// Shared by the `DELETE` route above and the idle reaper
/// (`idle_reaper::run`) — both go through this one path rather than a
/// second, drifted copy of stop logic, and both get the same
/// preserve-by-default behavior for the same reason: consistency between
/// an explicit stop and an automatic idle-timeout stop.
pub(crate) async fn stop_sandbox_by_id(state: Arc<AppState>, id: String, keep: bool) -> Result<StopOutcome, StopError> {
    if keep {
        match snapshot_and_stop(state.clone(), id.clone()).await {
            Ok(snapshot_id) => return Ok(StopOutcome::Snapshotted(snapshot_id)),
            Err(SnapshotStopError::NotFound) => return Err(StopError::NotFound),
            Err(SnapshotStopError::Io(e)) => return Err(StopError::Io(e)),
            Err(SnapshotStopError::Blocked(SnapshotBlocked::ForkedFrom(_))) => {
                // Falls through to the destroy below — see this
                // function's doc comment for why that's correct, not a
                // silent downgrade.
            }
            Err(SnapshotStopError::Blocked(reason @ SnapshotBlocked::Jailed)) => {
                return Err(StopError::CannotPreserve(reason));
            }
        }
    }
    destroy_sandbox_by_id(state, id).await
}

/// Removes a sandbox from the map and tears it down outright: VM stop,
/// network release, rootfs cleanup. The original (pre-naming-feature)
/// "stop a sandbox" behavior — now reached via `keep=false`, or
/// internally when `keep=true` has nothing new to preserve for a forked
/// sandbox (see `stop_sandbox_by_id`'s doc comment).
///
/// A sandbox forked from a snapshot (`source_snapshot_id.is_some()`,
/// see `routes_snapshot::fork_snapshot`) doesn't own its rootfs file or
/// network lease — both still belong to the snapshot, so they're neither
/// deleted nor released here. What it *does* release is the snapshot's
/// fork lock (`Snapshot::forked_into`), letting a later `/fork` or
/// `/resume` proceed — but only after `vm.stop()` returns, which kills
/// and waits on the Firecracker process: clearing the lock any earlier
/// would let a new fork start writing the shared rootfs file before the
/// old one has actually stopped touching it.
async fn destroy_sandbox_by_id(state: Arc<AppState>, id: String) -> Result<StopOutcome, StopError> {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or(StopError::NotFound)?;
    let source_snapshot_id = sandbox.source_snapshot_id.clone();
    let owns_rootfs = source_snapshot_id.is_none();

    // This live instance's pool membership (if any) ends here — see the
    // identical release in `routes_snapshot::snapshot_and_stop` for why
    // this doesn't carry forward onto anything resumed/forked later.
    if let Some(pool_id) = &sandbox.source_pool_id {
        if let Some(pool) = state.pools.lock().unwrap().get_mut(pool_id) {
            pool.record_release();
        }
    }

    spawn_blocking_in_current_span("stop task panicked", {
        let state = state.clone();
        move || {
            let _ = sandbox.vm.stop();
            if let Some(network) = sandbox.network {
                // Lease is about to be released back to the free pool --
                // this sandbox's dedicated chain (if any) has to go first,
                // matching the "removed only when the lease is finally
                // released" lifecycle. Safe to call even if no policy was
                // ever applied -- see `egress::remove`'s own doc comment.
                sandkiln_vmm::egress::remove(network.config.guest_ip, &network.config.tap_device, state.network.uplink());
                let _ = state.network.release(network);
            }
            if owns_rootfs {
                let _ = std::fs::remove_file(&sandbox.rootfs_path);
            }
            // `Vm::stop` already removed this sandbox's chroot directory if
            // it was jailed — this releases the daemon-level uid/gid
            // allocation, a separate resource `Vm` has no visibility into.
            if let (Some(id), Some(pool)) = (sandbox.jail_id, &state.jailer_ids) {
                pool.release(id);
            }
        }
    })
    .await;

    if let Some(snapshot_id) = source_snapshot_id {
        if let Some(snapshot) = state.snapshots.lock().unwrap().get_mut(&snapshot_id) {
            snapshot.forked_into = None;
        }
    }

    if let Err(e) = state.history.record_ended(&id, SystemTime::now(), sandkiln_store::EndReason::Destroyed, None) {
        tracing::warn!(error = %e, sandbox_id = %id, "failed to record sandbox destruction in history store");
    }

    Ok(StopOutcome::Destroyed)
}

/// Resolves a per-request resource override (`vcpu_count`/`mem_size_mib`
/// on `CreateSandboxRequest`) against the daemon's configured default and
/// ceiling. `None` (the field omitted) returns `default` unchanged —
/// today's behavior for a caller that doesn't ask for anything special. A
/// caller-supplied `0` (meaningless — a VM can't run with zero vCPUs or
/// zero memory) or anything above `max` is rejected outright rather than
/// silently clamped, so an unreasonable request fails loudly instead of
/// quietly running with less than the caller thought they'd get. A
/// negative value can't reach here at all: `vcpu_count`/`mem_size_mib`
/// deserialize as unsigned integers, so `serde_json` already rejects a
/// negative number in the request body before this is ever called.
pub(crate) fn resolve_resource_override<T>(requested: Option<T>, default: T, max: T, field: &str) -> Result<T, String>
where
    T: PartialOrd + Copy + Default + std::fmt::Display,
{
    match requested {
        None => Ok(default),
        Some(value) if value == T::default() => Err(format!("{field} must be greater than 0")),
        Some(value) if value > max => Err(format!("{field} {value} exceeds the configured maximum of {max}")),
        Some(value) => Ok(value),
    }
}

/// Resolves a per-request `rate_limit` into the vmm-level `RateLimiter`
/// Firecracker actually understands. `None` (the field omitted) means
/// unlimited I/O, unchanged from before this existed — returns `Ok(None)`.
/// A caller-supplied `rate_limit` with neither sub-field set, or either
/// set to `0`, is rejected outright (`0` bytes/s or ops/s is meaningless —
/// no drive could ever make progress) rather than silently treated as
/// unlimited, mirroring `resolve_resource_override`'s convention. Each
/// token bucket refills to its full `size` once per second
/// (`refill_time: 1000`ms) with no initial burst — the simplest possible
/// mapping from "bytes/ops per second" to Firecracker's bucket model;
/// burst tuning isn't exposed at this level yet.
fn resolve_rate_limit(requested: &Option<RateLimitRequest>) -> Result<Option<RateLimiter>, String> {
    let Some(req) = requested else { return Ok(None) };
    if req.bandwidth_bytes_per_sec.is_none() && req.ops_per_sec.is_none() {
        return Err("rate_limit must set at least one of bandwidth_bytes_per_sec/ops_per_sec".to_string());
    }
    let to_bucket = |field: &str, value: Option<u64>| -> Result<Option<TokenBucket>, String> {
        match value {
            None => Ok(None),
            Some(0) => Err(format!("rate_limit.{field} must be greater than 0")),
            Some(size) => Ok(Some(TokenBucket { size, one_time_burst: None, refill_time: 1000 })),
        }
    };
    Ok(Some(RateLimiter {
        bandwidth: to_bucket("bandwidth_bytes_per_sec", req.bandwidth_bytes_per_sec)?,
        ops: to_bucket("ops_per_sec", req.ops_per_sec)?,
    }))
}

/// Validates every CIDR in a requested egress policy up front — one clear
/// `400` naming the exact bad entry, rather than a cryptic iptables
/// failure surfacing later from deep inside a boot task.
fn resolve_egress_policy(requested: &Option<EgressPolicyRequest>) -> Result<Option<sandkiln_vmm::egress::EgressPolicy>, String> {
    let Some(req) = requested else { return Ok(None) };
    for cidr in req.allow_cidrs.iter().chain(&req.deny_cidrs) {
        sandkiln_vmm::egress::validate_cidr(cidr)?;
    }
    let mode = match req.mode {
        EgressModeRequest::AllowAll => sandkiln_vmm::egress::EgressMode::AllowAll,
        EgressModeRequest::DenyAll => sandkiln_vmm::egress::EgressMode::DenyAll,
    };
    Ok(Some(sandkiln_vmm::egress::EgressPolicy { mode, allow_cidrs: req.allow_cidrs.clone(), deny_cidrs: req.deny_cidrs.clone() }))
}

/// Returns the first item that's already been seen, if any.
fn first_duplicate<'a>(mut items: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut seen = HashSet::new();
    items.find(|item| !seen.insert(*item))
}

/// Clones the base rootfs for one sandbox. Uses `cp --reflink=auto`
/// rather than `std::fs::copy` so this becomes an instant copy-on-write
/// clone for free on a filesystem that supports it (XFS, Btrfs) — on
/// ext4 (what the dev box runs) `--reflink=auto` just falls back to an
/// ordinary copy, so this has no effect there, but costs nothing either.
fn clone_rootfs(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    let status = std::process::Command::new("cp").arg("--reflink=auto").arg(src).arg(dst).status()?;
    if !status.success() {
        return Err(std::io::Error::other(format!("cp --reflink=auto {src:?} {dst:?} failed: {status}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keep_defaults_to_true_when_absent() {
        assert_eq!(parse_keep(&HashMap::new()), Ok(true));
    }

    #[test]
    fn parse_keep_accepts_explicit_true_and_false() {
        assert_eq!(parse_keep(&HashMap::from([("keep".to_string(), "true".to_string())])), Ok(true));
        assert_eq!(parse_keep(&HashMap::from([("keep".to_string(), "false".to_string())])), Ok(false));
    }

    #[test]
    fn parse_keep_rejects_anything_else() {
        let err = parse_keep(&HashMap::from([("keep".to_string(), "yes".to_string())])).unwrap_err();
        assert!(err.contains("yes"), "message was: {err}");
    }

    #[test]
    fn cannot_preserve_error_for_jailed_mentions_the_opt_out() {
        let AppError::Conflict(message) = cannot_preserve_error(SnapshotBlocked::Jailed) else {
            panic!("expected Conflict")
        };
        assert!(message.contains("keep=false"), "message was: {message}");
    }

    #[test]
    fn resolve_resource_override_uses_the_default_when_omitted() {
        assert_eq!(resolve_resource_override(None, 2u8, 16u8, "vcpu_count"), Ok(2));
    }

    #[test]
    fn resolve_resource_override_accepts_a_value_within_the_ceiling() {
        assert_eq!(resolve_resource_override(Some(8u8), 2u8, 16u8, "vcpu_count"), Ok(8));
    }

    #[test]
    fn resolve_resource_override_accepts_a_value_exactly_at_the_ceiling() {
        assert_eq!(resolve_resource_override(Some(16u8), 2u8, 16u8, "vcpu_count"), Ok(16));
    }

    #[test]
    fn resolve_resource_override_rejects_zero() {
        assert_eq!(resolve_resource_override(Some(0u8), 2u8, 16u8, "vcpu_count"), Err("vcpu_count must be greater than 0".to_string()));
    }

    #[test]
    fn resolve_resource_override_rejects_above_the_ceiling() {
        assert_eq!(
            resolve_resource_override(Some(17u8), 2u8, 16u8, "vcpu_count"),
            Err("vcpu_count 17 exceeds the configured maximum of 16".to_string())
        );
    }

    #[test]
    fn resolve_resource_override_works_for_mem_size_mib_too() {
        assert_eq!(resolve_resource_override(Some(4096u32), 512u32, 16384u32, "mem_size_mib"), Ok(4096));
        assert_eq!(
            resolve_resource_override(Some(u32::MAX), 512u32, 16384u32, "mem_size_mib"),
            Err(format!("mem_size_mib {} exceeds the configured maximum of 16384", u32::MAX))
        );
    }

    #[test]
    fn first_duplicate_finds_a_repeat() {
        assert_eq!(first_duplicate(["a", "b", "a"].into_iter()), Some("a"));
        assert_eq!(first_duplicate(["a", "b", "c", "b"].into_iter()), Some("b"));
    }

    #[test]
    fn first_duplicate_none_when_all_unique() {
        assert_eq!(first_duplicate(["a", "b", "c"].into_iter()), None);
        assert_eq!(first_duplicate(std::iter::empty()), None);
    }

    #[test]
    fn resolve_egress_policy_returns_none_when_omitted() {
        assert_eq!(resolve_egress_policy(&None), Ok(None));
    }

    #[test]
    fn resolve_egress_policy_accepts_well_formed_cidrs_in_both_lists() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::DenyAll,
            allow_cidrs: vec!["10.0.0.0/8".to_string()],
            deny_cidrs: vec!["192.168.1.1/32".to_string()],
        });
        let policy = resolve_egress_policy(&requested).unwrap().unwrap();
        assert_eq!(policy.mode, sandkiln_vmm::egress::EgressMode::DenyAll);
        assert_eq!(policy.allow_cidrs, vec!["10.0.0.0/8".to_string()]);
        assert_eq!(policy.deny_cidrs, vec!["192.168.1.1/32".to_string()]);
    }

    #[test]
    fn resolve_egress_policy_rejects_a_malformed_allow_cidr() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::AllowAll,
            allow_cidrs: vec!["not-a-cidr".to_string()],
            deny_cidrs: vec![],
        });
        let err = resolve_egress_policy(&requested).unwrap_err();
        assert!(err.contains("not-a-cidr"), "message was: {err}");
    }

    #[test]
    fn resolve_egress_policy_rejects_a_malformed_deny_cidr() {
        let requested = Some(EgressPolicyRequest {
            mode: EgressModeRequest::AllowAll,
            allow_cidrs: vec![],
            deny_cidrs: vec!["10.0.0.0/33".to_string()],
        });
        let err = resolve_egress_policy(&requested).unwrap_err();
        assert!(err.contains("10.0.0.0/33"), "message was: {err}");
    }

    #[test]
    fn clone_rootfs_copies_real_file_contents() {
        let dir = std::env::temp_dir().join(format!("sandkiln-clone-rootfs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, b"some rootfs bytes").unwrap();

        clone_rootfs(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"some rootfs bytes");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clone_rootfs_fails_cleanly_for_a_missing_source() {
        let dir = std::env::temp_dir().join(format!("sandkiln-clone-rootfs-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = clone_rootfs(&dir.join("does-not-exist.bin"), &dir.join("dst.bin"));
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

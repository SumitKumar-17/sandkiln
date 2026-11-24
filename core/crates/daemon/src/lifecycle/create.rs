//! Sandbox creation: the boot mechanics shared by `POST /sandboxes` and
//! `POST /sandboxes/get-or-create`.
//!
//! Owns `create_sandbox_core` (request validation against drives and limits,
//! pool claim, cold-create fallback), `create_sandbox_cold` (concurrent rootfs
//! clone + network lease, boot, egress, history, per-phase timing),
//! `PendingImageBootGuard` (keeps a concurrent image delete from racing the
//! clone) and `clone_rootfs`, the one way any mutating sandbox gets its own
//! disk. Failure paths release what was allocated: slot, lease, clone.

use crate::http::error::AppError;
use crate::http::routes::sandbox::CreateSandboxRequest;
use crate::lifecycle::create_options::{resolve_egress_policy, resolve_rate_limit, resolve_resource_override, first_duplicate};
use crate::lifecycle::pool_claim::{claim_from_pool, resolve_pool_claim, MAX_POOL_CLAIM_ATTEMPTS, PoolClaim, PoolClaimGuard};
use crate::metrics::{CreatePhase, PoolClaimOutcome};
use crate::sandbox::Sandbox;
use crate::state::{can_attach_read_only, describe_drive_holders, AppState, AttachedDrive};
use crate::tracing_util::spawn_blocking_in_current_span;
use sandkiln_vmm::jailer::JailLaunch;
use sandkiln_vmm::network::Lease;
use sandkiln_vmm::vm::{DriveConfig, RateLimiter, Vm, VmConfig};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use uuid::Uuid;

/// RAII for `AppState::reserve_pending_image_boot`/
/// `release_pending_image_boot` — releases on drop, covering every exit
/// path (happy path, early `?`, panic unwind). No-op when `image_id` is
/// `None`.
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

/// Shared boot mechanics for `create_sandbox` and
/// `routes::sandbox_name::get_or_create_sandbox`'s create-fresh path.
/// Doesn't check name uniqueness — both callers already did, under the
/// same `lock_name` guard still held here.
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

    // `drives`/`rate_limit` are baked into a VM at boot time; a warm
    // snapshot was booted with neither, so either present here means
    // this can never match a pool (see `crate::lifecycle::pool`) — falls through
    // to cold-create and never queues either, for the same reason.
    if request.drives.is_empty() && request.rate_limit.is_none() {
        let key = crate::lifecycle::pool::PoolKey { image_id: request.image_id.clone(), vcpu_count, mem_size_mib };
        // Bounded retry: a warm claim failing its post-resume health
        // check (historically up to 2 in 3 resumes, rare now: see `claim_from_pool`) releases its
        // slot via `PoolClaimGuard`'s drop, and re-resolving immediately
        // is what keeps the fallback cold-create still counted against
        // `max_count` instead of silently exceeding it. Bounded only as
        // a safety margin against a pathological run of bad warm
        // snapshots — see `MAX_POOL_CLAIM_ATTEMPTS`.
        for _ in 0..MAX_POOL_CLAIM_ATTEMPTS {
            match resolve_pool_claim(state, &key).await? {
                PoolClaim::Warm { pool_id, snapshot_id } => {
                    let guard = PoolClaimGuard::new(state.clone(), Some(pool_id.clone()));
                    match claim_from_pool(state, snapshot_id, pool_id, &request, egress.clone()).await {
                        Ok(id) => {
                            guard.commit();
                            state.metrics.record_pool_claim(PoolClaimOutcome::Warm);
                            return Ok(id);
                        }
                        Err(e) => {
                            // `guard` drops uncommitted here, freeing the
                            // slot for the next loop iteration.
                            state.metrics.record_pool_claim(PoolClaimOutcome::WarmFailed);
                            tracing::warn!(error = %e, "pool claim failed — retrying against the pool once more before falling back unattributed");
                        }
                    }
                }
                PoolClaim::ColdSlot { pool_id } => {
                    // Room under max_count, nothing warm ready — cold-create
                    // below, attributed to this pool.
                    state.metrics.record_pool_claim(PoolClaimOutcome::ColdSlot);
                    return create_sandbox_cold(state, request, Some(pool_id), vcpu_count, mem_size_mib, rate_limit, egress).await;
                }
                PoolClaim::NoPool => break, // no pool configured for this profile.
            }
        }
    }

    create_sandbox_cold(state, request, None, vcpu_count, mem_size_mib, rate_limit, egress).await
}

/// Boots one instance for a pool's background replenishment. Goes straight to
/// the cold path: through `create_sandbox_core` the request would match the
/// very pool being filled and resume that pool's own warm snapshot, so a
/// pool wanting 2 would stay at 1 forever.
pub(crate) async fn create_warm_instance(state: &Arc<AppState>, request: CreateSandboxRequest) -> Result<String, AppError> {
    let vcpu_count = resolve_resource_override(request.vcpu_count, state.config.vcpu_count, state.config.max_vcpu_count, "vcpu_count")
        .map_err(AppError::BadRequest)?;
    let mem_size_mib =
        resolve_resource_override(request.mem_size_mib, state.config.mem_size_mib, state.config.max_mem_size_mib, "mem_size_mib")
            .map_err(AppError::BadRequest)?;
    create_sandbox_cold(state, request, None, vcpu_count, mem_size_mib, None, None).await
}

/// Cold-boot mechanics. `pool_id` is `Some` when `create_sandbox_core`
/// reserved a `max_count` slot (`PoolClaim::ColdSlot`) — released on
/// failure via `PoolClaimGuard`, committed to `Sandbox::source_pool_id`
/// on success.
async fn create_sandbox_cold(
    state: &Arc<AppState>,
    request: CreateSandboxRequest,
    pool_id: Option<String>,
    vcpu_count: u8,
    mem_size_mib: u32,
    rate_limit: Option<RateLimiter>,
    egress: Option<sandkiln_vmm::egress::EgressPolicy>,
) -> Result<String, AppError> {
    let create_started = Instant::now();
    let pool_guard = PoolClaimGuard::new(state.clone(), pool_id.clone());

    // Reserved before the (slow) boot starts, closing the window where a
    // concurrent `DELETE /images/:id` could remove the file this boot's
    // rootfs copy is reading. `_image_boot_guard` releases it on every
    // exit path.
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
    // Guest-reachable via Firecracker's own MMDS (see VmConfig::metadata).
    let metadata = serde_json::json!({ "id": id, "name": &request.name, "tags": &request.tags });
    let rootfs_path = crate::storage::rootfs_path(&id);
    let attached_drives: Vec<AttachedDrive> =
        request.drives.iter().map(|d| AttachedDrive { drive_id: d.id.clone(), read_only: d.read_only }).collect();
    // Prefixed to stay distinct from the reserved "rootfs" id. Firecracker
    // only allows alphanumerics/underscores in a drive_id, so '-' (these
    // are UUIDs) becomes '_'.
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
            // Rootfs copy and network lease run concurrently. The clone
            // dominates (~124ms vs ~4ms on this dev box's ext4), so the
            // concurrency mostly hides nothing today — it stays because
            // it's free and pays off once `cp --reflink=auto` can
            // actually reflink (see `clone_rootfs`). Note the
            // destination is `std::env::temp_dir()`: reflink silently
            // degrades to a full copy across filesystems, the leading
            // (unverified) explanation for why an XFS experiment showed
            // no end-to-end win — see `docs/benchmarks.md`.
            let setup_started = Instant::now();
            let ((copy_result, copy_elapsed), (lease_result, lease_elapsed)) = std::thread::scope(|scope| {
                let copy_handle = scope.spawn(|| span.in_scope(|| timed(|| clone_rootfs(&base_rootfs_source, &rootfs_path, &state.metrics))));
                let lease_handle = scope.spawn(|| span.in_scope(|| timed(|| state.network.lease())));
                (copy_handle.join().expect("rootfs copy thread panicked"), lease_handle.join().expect("lease thread panicked"))
            });
            let setup_elapsed = setup_started.elapsed();
            // Recorded before the `?`s so a create that fails mid-phase
            // still contributes the timing it produced.
            state.metrics.record_create_phase_ms(CreatePhase::RootfsClone, ms(copy_elapsed));
            state.metrics.record_create_phase_ms(CreatePhase::NetworkLease, ms(lease_elapsed));
            state.metrics.record_create_phase_ms(CreatePhase::Setup, ms(setup_elapsed));
            tracing::debug!(
                rootfs_clone_us = copy_elapsed.as_micros(),
                network_lease_us = lease_elapsed.as_micros(),
                setup_join_us = setup_elapsed.as_micros(),
                "cold create setup phases"
            );
            // The two halves ran concurrently, so either can succeed while the
            // other fails: unwind whichever one holds a resource.
            let lease = match (copy_result, lease_result) {
                (Ok(()), Ok(lease)) => lease,
                (copy, lease) => {
                    if copy.is_ok() {
                        let _ = std::fs::remove_file(&rootfs_path);
                    }
                    let lease_err = match lease {
                        Ok(lease) => {
                            state.network.release(lease);
                            None
                        }
                        Err(e) => Some(e),
                    };
                    return Err(copy.err().or(lease_err).expect("one half failed"));
                }
            };
            // From here until the VM is booted, every early return removes the
            // rootfs clone; `keep()` below hands it to the live sandbox.
            let mut rootfs_guard = crate::storage::OwnedFile::new(rootfs_path.clone());

            // Third resource besides rootfs/lease: an in-memory pool pop,
            // leased last (nothing to overlap it with), released
            // immediately on failure just like a failed `Vm::boot`
            // releases the network lease below.
            let jail_id = match &state.jailer_ids {
                Some(pool) => match pool.lease() {
                    Ok(id) => Some(id),
                    Err(e) => {
                        state.network.release(lease);
                        return Err(e);
                    }
                },
                None => None,
            };
            let jail = jail_id.and_then(|id| {
                state.config.jailer.as_ref().map(|j| JailLaunch {
                    jailer_bin: j.jailer_bin.clone(),
                    supervisor_bin: j.supervisor_bin.clone(),
                    chroot_base_dir: j.chroot_base_dir.clone(),
                    uid: id,
                    gid: j.gid,
                })
            });

            let boot_started = Instant::now();
            let vm = Vm::boot(&VmConfig {
                firecracker_bin: state.config.firecracker_bin.clone(),
                kernel_path: state.config.kernel_path.clone(),
                rootfs_path,
                vcpu_count,
                mem_size_mib,
                extra_boot_args: state.config.kernel_args.clone(),
                network: Some(lease.config.clone()),
                extra_drives,
                jail,
                rate_limit,
                metadata: Some(metadata),
            });
            match vm {
                Ok(vm) => {
                    state.metrics.record_boot_duration_ms(ms(boot_started.elapsed()));
                    // Treated like a failed `Vm::boot`: a policy that
                    // fails to apply must not leave the sandbox silently
                    // unrestricted, so tear down and return the error.
                    if let Some(policy) = &egress {
                        let egress_started = Instant::now();
                        let egress_result =
                            sandkiln_vmm::egress::apply(lease.config.guest_ip, &lease.config.tap_device, state.network.uplink(), policy);
                        state.metrics.record_create_phase_ms(CreatePhase::EgressApply, ms(egress_started.elapsed()));
                        if let Err(e) = egress_result {
                            let _ = vm.stop();
                            // `apply` can fail after installing the chain.
                            sandkiln_vmm::egress::remove(lease.config.guest_ip, &lease.config.tap_device, state.network.uplink());
                            state.network.release(lease);
                            if let (Some(id), Some(pool)) = (jail_id, &state.jailer_ids) {
                                pool.release(id);
                            }
                            return Err(e);
                        }
                    }
                    rootfs_guard.keep();
                    Ok((vm, lease, jail_id))
                }
                Err(e) => {
                    state.network.release(lease);
                    if let (Some(id), Some(pool)) = (jail_id, &state.jailer_ids) {
                        // A process that survived the failed boot still runs
                        // as this uid: never hand the uid to another VM.
                        if sandkiln_vmm::vm::is_unkilled_process(&e) {
                            pool.quarantine(id);
                        } else {
                            pool.release(id);
                        }
                    }
                    Err(e)
                }
            }
        }
    })
    .await?;
    let boot_task_elapsed = create_started.elapsed();

    let created_at = SystemTime::now();
    // Best-effort: the VM already booted, so a history-write failure
    // shouldn't fail the whole request — only warn.
    let before_history = Instant::now();
    if let Err(e) = state.history.record_created(&id, request.name.as_deref(), &request.tags, request.image_id.as_deref(), created_at) {
        tracing::warn!(error = %e, sandbox_id = %id, "failed to record sandbox creation in history store");
    }
    let history_elapsed = before_history.elapsed();

    let sandbox = Sandbox {
        id: id.clone(),
        vm,
        network: Some(network),
        fork_network: None,
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
        env: request.env,
        // A cold create (fresh or a pool's own warm boot) is always a
        // lineage root.
        parent_snapshot_id: None,
        mounts: Vec::new(),
        log_sessions: Default::default(),
    };
    state.sandboxes.lock().unwrap().insert(id.clone(), sandbox);
    state.metrics.record_sandbox_created();
    // The reserved slot is now durably owned by the live `Sandbox` via
    // `source_pool_id` — released later by destroy/snapshot-and-stop,
    // not this guard.
    pool_guard.commit();

    let total = create_started.elapsed();
    state.metrics.record_create_phase_ms(CreatePhase::Total, ms(total));
    // `boot_task_us` covers the whole spawn_blocking closure; the gap to
    // `total` is the daemon-side tail (history write, map lock) plus
    // blocking-pool wait time.
    tracing::debug!(
        sandbox_id = %id,
        boot_task_us = boot_task_elapsed.as_micros(),
        history_write_us = history_elapsed.as_micros(),
        total_us = total.as_micros(),
        "cold create complete"
    );

    Ok(id)
}

/// Runs `f`, returning its value plus elapsed time — lets each
/// concurrently-spawned setup phase time itself, since a timer around
/// the join alone can't tell the two apart.
fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = f();
    (value, started.elapsed())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Clones a rootfs for a new mutating sandbox via
/// `sandkiln_vmm::rootfs::clone_file` (reflink where the filesystem allows,
/// else a byte copy) and records which method ran. `pub(crate)`: also used by
/// `snapshots::ops`'s history-retaining resume and fork, and
/// `routes::snapshot_history`'s restore — handing a *shared* rootfs to a new
/// mutating sandbox would corrupt whatever else depends on that file.
pub(crate) fn clone_rootfs(
    src: &std::path::Path,
    dst: &std::path::Path,
    metrics: &crate::metrics::Metrics,
) -> std::io::Result<()> {
    let method = sandkiln_vmm::rootfs::clone_file(src, dst)?;
    metrics.record_rootfs_clone(method);
    tracing::debug!(method = method.as_str(), ?src, ?dst, "rootfs cloned");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_rootfs_copies_real_file_contents() {
        let dir = std::env::temp_dir().join(format!("sandkiln-clone-rootfs-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.bin");
        let dst = dir.join("dst.bin");
        std::fs::write(&src, b"some rootfs bytes").unwrap();

        clone_rootfs(&src, &dst, &crate::metrics::Metrics::new()).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"some rootfs bytes");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clone_rootfs_fails_cleanly_for_a_missing_source() {
        let dir = std::env::temp_dir().join(format!("sandkiln-clone-rootfs-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = clone_rootfs(&dir.join("does-not-exist.bin"), &dir.join("dst.bin"), &crate::metrics::Metrics::new());
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

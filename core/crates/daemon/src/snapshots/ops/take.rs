//! Take a snapshot: pause a live sandbox, write its state and memory to disk,
//! stop the VM, and replace the `Sandbox` with a held `Snapshot`.
//!
//! Owns `snapshot_and_stop`, the single definition of that mechanic shared by
//! the explicit snapshot route, `stop_sandbox_by_id` (preserve-by-default) and
//! the idle reaper's auto-suspend, plus the structural preconditions
//! (`check_snapshottable`) and the error types its callers react to.

use crate::http::error::AppError;
use crate::sandbox::Sandbox;
use crate::snapshots::held::Snapshot;
use crate::snapshots::layout::snapshot_dir;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use sandkiln_vmm::network::IpLease;
use std::sync::Arc;
use std::time::SystemTime;
use uuid::Uuid;

/// Why a sandbox can't be snapshotted — the two structural (not
/// transient) reasons `check_snapshottable` refuses. Split from
/// `SnapshotStopError` so each caller reacts differently:
/// `snapshot_sandbox` turns either into an error, while
/// `lifecycle::stop::stop_sandbox_by_id` treats `ForkedFrom` as harmless
/// and only `Jailed` as a real conflict.
pub(crate) enum SnapshotBlocked {
    /// Firecracker's snapshot state bakes in the in-jail paths a jailed
    /// chroot used; `Vm::resume` only ever spawns directly, so it'd try
    /// those paths against the host's real root — see
    /// `sandkiln_vmm::jailer`.
    Jailed,
    /// Forked from the named snapshot, sharing its rootfs rather than
    /// owning a copy — snapshotting it would cascade the exact
    /// resume-time conflict `forked_into` exists to prevent.
    ForkedFrom(String),
}

/// Pure precondition behind `snapshot_and_stop`, pulled out for direct
/// unit testing (mirrors `check_no_live_fork` below).
fn check_snapshottable(is_jailed: bool, source_snapshot_id: Option<&str>) -> Result<(), SnapshotBlocked> {
    if is_jailed {
        return Err(SnapshotBlocked::Jailed);
    }
    if let Some(source) = source_snapshot_id {
        return Err(SnapshotBlocked::ForkedFrom(source.to_string()));
    }
    Ok(())
}

/// Every way `snapshot_and_stop` can fail to produce a `Snapshot`.
pub(crate) enum SnapshotStopError {
    NotFound,
    Blocked(SnapshotBlocked),
    Io(std::io::Error),
}

impl From<SnapshotStopError> for AppError {
    /// Used by `snapshot_sandbox` (a direct, explicit ask with no
    /// fallback — both `Blocked` reasons become real errors).
    /// `stop_sandbox_by_id` does NOT use this: it treats `ForkedFrom` as
    /// non-error and matches on `SnapshotStopError` directly.
    fn from(e: SnapshotStopError) -> Self {
        match e {
            SnapshotStopError::NotFound => AppError::NotFound(String::new()),
            SnapshotStopError::Blocked(SnapshotBlocked::Jailed) => {
                AppError::BadRequest("snapshotting a jailed sandbox is not supported yet".to_string())
            }
            SnapshotStopError::Blocked(SnapshotBlocked::ForkedFrom(source)) => AppError::Conflict(format!(
                "this sandbox was forked from snapshot {source} and shares its rootfs file — stop it and fork \
                 {source} again instead of snapshotting it directly"
            )),
            SnapshotStopError::Io(e) => AppError::from(e),
        }
    }
}

/// Pauses the VM, snapshots to disk, stops the process — the sandbox
/// stops existing as a live `Sandbox` and a `Snapshot` (same `name`, if
/// any) takes its place. The rootfs and the guest's IP aren't
/// released/removed (unlike a full destroy): both move to the
/// `Snapshot` for `resume_snapshot_by_id`/`fork_snapshot` to hand on. The
/// **tap** goes back to the pool the moment the VM is dead — a held snapshot
/// must not pin one — and so does the sandbox's egress chain, which is keyed
/// by that tap; resume leases a free tap and re-applies the policy to it.
///
/// Shared by `snapshot_sandbox` (explicit), `stop_sandbox_by_id`
/// (preserve-by-default), and `idle_reaper`'s auto-suspend — one place
/// owns the mechanics so the three can't drift.
pub(crate) async fn snapshot_and_stop(state: Arc<AppState>, id: String) -> Result<String, SnapshotStopError> {
    // Checked before removing from the map, so a rejected request leaves
    // the sandbox exactly as it was.
    {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or(SnapshotStopError::NotFound)?;
        check_snapshottable(sandbox.vm.is_jailed(), sandbox.source_snapshot_id.as_deref())
            .map_err(SnapshotStopError::Blocked)?;
    }

    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or(SnapshotStopError::NotFound)?;
    // Pool membership ends here regardless of outcome — a later
    // resume/fork is a fresh creation event, not tied back to this pool
    // (see `crate::lifecycle::pool`).
    if let Some(pool_id) = &sandbox.source_pool_id {
        if let Some(pool) = state.pools.lock().unwrap().get_mut(pool_id) {
            pool.record_release();
        }
    }
    let Sandbox { vm, network, rootfs_path, attached_drives, image_id, tags, name, egress, env, parent_snapshot_id, mounts, .. } =
        sandbox;
    // Only a forked descendant (rejected above) ever has `network: None`.
    let network = network.expect("non-fork sandboxes always hold a network lease");

    let has_egress_chain = egress.is_some();
    let snapshot_id = Uuid::new_v4().to_string();
    let dir = snapshot_dir(&snapshot_id);
    let snapshot_path = dir.join("state.snap");
    let mem_file_path = dir.join("mem.bin");

    let result = spawn_blocking_in_current_span("snapshot task panicked", {
        let state = state.clone();
        let dir = dir.clone();
        let snapshot_path = snapshot_path.clone();
        let mem_file_path = mem_file_path.clone();
        move || -> std::io::Result<IpLease> {
            let outcome = std::fs::create_dir_all(&dir)
                .and_then(|_| vm.pause())
                .and_then(|_| vm.snapshot(&mem_file_path, &snapshot_path));
            // Either way this VM is done — a paused VM that failed to
            // snapshot can't be handed back as still-running. `force_stop`,
            // not `stop`: stop() first sends a guest `sync`, which a paused
            // guest can never answer, so it only added a ~6s timeout to every
            // snapshot (6.8s measured end to end, ~0.5s of real work).
            let _ = vm.force_stop();
            // The guest is dead, so its tap is free and its egress chain
            // (keyed by that tap) is moot. Both go now whatever happened.
            if has_egress_chain {
                sandkiln_vmm::egress::remove(network.config.guest_ip, &network.config.tap_device, state.network.uplink());
            }
            match outcome {
                Ok(()) => Ok(state.network.detach(network)),
                Err(e) => {
                    state.network.release(network);
                    Err(e)
                }
            }
        }
    })
    .await;

    let network = match result {
        Ok(ip) => ip,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::remove_file(&rootfs_path);
            return Err(SnapshotStopError::Io(e));
        }
    };

    let snapshot = Snapshot {
        id: snapshot_id.clone(),
        source_sandbox_id: id.clone(),
        snapshot_path,
        mem_file_path,
        rootfs_path,
        network,
        attached_drives,
        image_id,
        tags,
        created_at: SystemTime::now(),
        name,
        forked_into: None,
        archived_at: None,
        egress,
        env,
        // `None` for a cold-booted sandbox (root of its own lineage).
        // Deliberately `parent_snapshot_id`, not `source_snapshot_id` —
        // the latter is `None` on resume by design (see its own doc
        // comment).
        parent_snapshot_id,
        mounts,
    };

    // Persisted before this snapshot is visible in `AppState` at all:
    // state.snap/mem.bin already exist on disk, so a metadata-write
    // failure here gets the same full teardown a failed snapshot itself
    // gets, not a `Snapshot` left alive with a broken durability
    // contract.
    let (snapshot, persist_result) = tokio::task::spawn_blocking(move || {
        let persist_result = snapshot.persist(&snapshot_dir(&snapshot.id));
        (snapshot, persist_result)
    })
    .await
    .expect("persist snapshot metadata task panicked");

    if let Err(e) = persist_result {
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup_state = state.clone();
        tokio::task::spawn_blocking(move || {
            cleanup_state.network.release_ip(snapshot.network);
            let _ = std::fs::remove_file(&snapshot.rootfs_path);
        })
        .await
        .expect("cleanup task panicked");
        return Err(SnapshotStopError::Io(e));
    }

    state.snapshots.lock().unwrap().insert(snapshot_id.clone(), snapshot);

    if let Err(e) = state.history.record_ended(&id, SystemTime::now(), sandkiln_store::EndReason::Snapshotted, Some(&snapshot_id)) {
        tracing::warn!(error = %e, sandbox_id = %id, "failed to record sandbox snapshot in history store");
    }

    Ok(snapshot_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_snapshottable_allows_an_unjailed_non_fork_sandbox() {
        assert!(check_snapshottable(false, None).is_ok());
    }

    #[test]
    fn check_snapshottable_blocks_a_jailed_sandbox() {
        let err = check_snapshottable(true, None).unwrap_err();
        assert!(matches!(err, SnapshotBlocked::Jailed));
    }

    #[test]
    fn check_snapshottable_blocks_a_forked_sandbox_and_names_its_source() {
        let err = check_snapshottable(false, Some("snap-parent")).unwrap_err();
        let SnapshotBlocked::ForkedFrom(source) = err else { panic!("expected ForkedFrom") };
        assert_eq!(source, "snap-parent");
    }

    #[test]
    fn check_snapshottable_prefers_the_jailed_reason_when_both_apply() {
        // Can't actually happen (a jailed boot never sets
        // `source_snapshot_id`, resume/fork never jail) — pins a
        // deterministic precedence anyway in case that ever changes.
        let err = check_snapshottable(true, Some("snap-parent")).unwrap_err();
        assert!(matches!(err, SnapshotBlocked::Jailed));
    }

    #[test]
    fn snapshot_stop_error_jailed_maps_to_bad_request_mentioning_unsupported() {
        let app_err: AppError = SnapshotStopError::Blocked(SnapshotBlocked::Jailed).into();
        let AppError::BadRequest(message) = app_err else { panic!("expected BadRequest") };
        assert!(message.contains("jailed"), "message was: {message}");
    }

    #[test]
    fn snapshot_stop_error_forked_from_maps_to_conflict_naming_the_source() {
        let app_err: AppError = SnapshotStopError::Blocked(SnapshotBlocked::ForkedFrom("snap-1".to_string())).into();
        let AppError::Conflict(message) = app_err else { panic!("expected Conflict") };
        assert!(message.contains("snap-1"), "message was: {message}");
    }
}

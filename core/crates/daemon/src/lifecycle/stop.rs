//! Stop and destroy a live sandbox.
//!
//! Owns `stop_sandbox_by_id`, the shared stop path behind `DELETE` and the
//! idle reaper and shutdown (snapshot-then-stop by default, destroy for
//! `keep=false` or when there is nothing to preserve), `destroy_sandbox_by_id`
//! (full teardown: VM, lease, egress chain, rootfs, jail id, fork lock) and the
//! HTTP-free `StopOutcome`/`StopError` result types.

use crate::snapshots::ops::take::{snapshot_and_stop, SnapshotBlocked, SnapshotStopError};
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use std::sync::Arc;
use std::time::SystemTime;

/// What `stop_sandbox_by_id` did — shapes `DELETE`'s response;
/// `idle_reaper::run` only cares whether it succeeded.
pub(crate) enum StopOutcome {
    Snapshotted(String),
    Destroyed,
}

/// Every way `stop_sandbox_by_id` can fail. Separate from `AppError` so
/// `idle_reaper` can react to `CannotPreserve` (fall back to a full
/// destroy) without an HTTP-shaped type.
pub(crate) enum StopError {
    NotFound,
    /// `keep=true` requested but this sandbox structurally can't be
    /// snapshotted right now (see `SnapshotBlocked`). A forked sandbox
    /// never reaches here — that's a silent, correct destroy instead
    /// (nothing new to preserve); only a jailed sandbox does.
    CannotPreserve(SnapshotBlocked),
    Io(std::io::Error),
}

/// `keep=true` (the default, for both `DELETE` and `idle_reaper`) is
/// "persistent by default": pause, snapshot to disk, stop — landing a
/// `Snapshot` record (with `name`, if any) instead of deleting rootfs
/// and releasing the lease. `keep=false` is the full-destruction opt-out
/// (e.g. a short-lived CI sandbox that won't come back).
///
/// A forked sandbox under `keep=true` is a special case: it shares its
/// source snapshot's rootfs rather than owning a copy, so it can't be
/// snapshotted again (`SnapshotBlocked::ForkedFrom`) — but that's fine,
/// not an error, since that shared snapshot already *is* this identity's
/// durable state. Destroying just the fork (which never touches the
/// shared rootfs/network — see `destroy_sandbox_by_id`) already
/// satisfies `keep=true`'s intent. A jailed sandbox has no such
/// fallback (jailed snapshot/resume genuinely isn't supported), so that
/// surfaces as `StopError::CannotPreserve` instead of silently
/// destroying state the caller expected to survive.
///
/// Shared by the `DELETE` route and `idle_reaper::run` so an explicit
/// stop and an automatic idle-timeout stop behave identically.
pub(crate) async fn stop_sandbox_by_id(state: Arc<AppState>, id: String, keep: bool) -> Result<StopOutcome, StopError> {
    if keep {
        match snapshot_and_stop(state.clone(), id.clone()).await {
            Ok(snapshot_id) => return Ok(StopOutcome::Snapshotted(snapshot_id)),
            Err(SnapshotStopError::NotFound) => return Err(StopError::NotFound),
            Err(SnapshotStopError::Io(e)) => return Err(StopError::Io(e)),
            Err(SnapshotStopError::Blocked(SnapshotBlocked::ForkedFrom(_))) => {
                // Falls through to destroy below — correct, not a silent
                // downgrade (see doc comment above).
            }
            Err(SnapshotStopError::Blocked(reason @ SnapshotBlocked::Jailed)) => {
                return Err(StopError::CannotPreserve(reason));
            }
        }
    }
    destroy_sandbox_by_id(state, id).await
}

/// Full teardown: VM stop, network release, rootfs cleanup. Reached via
/// `keep=false`, or internally when `keep=true` has nothing new to
/// preserve for a fork.
///
/// A forked sandbox (`source_snapshot_id.is_some()`) doesn't own its
/// rootfs or lease — both still belong to the snapshot, so neither is
/// touched here. What it does release is the snapshot's fork lock
/// (`Snapshot::forked_into`), unblocking a later `/fork`/`/resume` — but
/// only after `vm.stop()` returns (kills and waits on the process), so a
/// new fork can't start writing the shared rootfs before the old one has
/// actually stopped touching it.
async fn destroy_sandbox_by_id(state: Arc<AppState>, id: String) -> Result<StopOutcome, StopError> {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or(StopError::NotFound)?;
    let source_snapshot_id = sandbox.source_snapshot_id.clone();

    // Pool membership ends here — see the identical release in
    // `snapshot_and_stop` for why it doesn't carry onto anything
    // resumed/forked later.
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
                // This sandbox's egress chain (if any) goes before the
                // lease is released, matching "removed only when the
                // lease is released." Safe even if no policy was ever
                // applied — see `egress::remove`.
                sandkiln_vmm::egress::remove(network.config.guest_ip, &network.config.tap_device, state.network.uplink());
                let _ = state.network.release(network);
            }
            // Every `Sandbox` owns a private rootfs outright (a fork
            // gets its own clone at fork time — see
            // `fork_snapshot`), so nothing else is left dangling by
            // removing this file.
            let _ = std::fs::remove_file(&sandbox.rootfs_path);
            // `Vm::stop` already removed the chroot dir if jailed; this
            // releases the separate daemon-level uid/gid allocation.
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

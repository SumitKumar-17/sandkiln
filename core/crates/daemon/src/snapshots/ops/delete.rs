//! Delete a held snapshot outright.
//!
//! Owns `delete_snapshot_by_id`: releases the snapshot's lease and egress
//! chain and removes its rootfs copy and state/memory files, refused while a
//! fork is alive.

use super::check_no_live_fork;
use crate::http::error::AppError;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use std::sync::Arc;

/// The actual teardown, shared by `delete_snapshot` and `crate::lifecycle::pool`'s
/// pool-deletion path (tearing down whatever a pool still has warm) —
/// same `_by_id` split as `resume_snapshot_by_id`/`stop_sandbox_by_id`.
pub(crate) async fn delete_snapshot_by_id(state: Arc<AppState>, id: String) -> Result<(), AppError> {
    let snapshot = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let existing = snapshots.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        check_no_live_fork(existing.forked_into.as_deref(), &id, "deleting")?;
        snapshots.remove(&id).expect("just checked it exists")
    };

    spawn_blocking_in_current_span("delete task panicked", {
        let state = state.clone();
        move || {
            // Same lease-release-tied teardown as `destroy_sandbox_by_id`.
            sandkiln_vmm::egress::remove(snapshot.network.config.guest_ip, &snapshot.network.config.tap_device, state.network.uplink());
            let _ = state.network.release(snapshot.network);
            let _ = std::fs::remove_file(&snapshot.rootfs_path);
            // `snapshot.snapshot_path`'s own parent, not a hardcoded hot
            // dir — an archived snapshot lives under `Config::archive_dir`
            // instead; removing the wrong directory would leak the real
            // files. Accurate for a hot snapshot too (same path either way).
            if let Some(dir) = snapshot.snapshot_path.parent() {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    })
    .await;

    Ok(())
}

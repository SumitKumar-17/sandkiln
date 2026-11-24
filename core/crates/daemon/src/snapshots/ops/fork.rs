//! Fork a held snapshot into a new live sandbox without consuming it.
//!
//! Owns `fork_snapshot_by_id`: reserves the single live-descendant slot
//! (`Snapshot::forked_into`), clones the rootfs before resuming so the
//! snapshot's own disk is never written, and releases the reservation on any
//! failure.

use super::check_no_live_fork;
use super::resume::resume_vm;
use crate::http::error::AppError;
use crate::sandbox::{ForkNetwork, Sandbox};
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use std::sync::Arc;
use std::time::SystemTime;
use uuid::Uuid;

/// Boots a new sandbox from a snapshot *without* consuming it — record
/// and files untouched, so it can be forked/resumed again later. The
/// forked sandbox doesn't own the snapshot's IP (stays with the `Snapshot`) and
/// has its own rootfs clone and tap; `stop_sandbox_by_id` returns the tap.
///
/// 409 while an earlier fork is still alive (see `Snapshot::forked_into`).
/// The new id is reserved up front, before the slow resume call, so a
/// second concurrent `/fork` sees the reservation and 409s immediately
/// instead of racing another `Vm::resume` onto the same rootfs.
pub(crate) async fn fork_snapshot_by_id(state: Arc<AppState>, snapshot_id: String) -> Result<String, AppError> {
    let new_id = Uuid::new_v4().to_string();

    let (snapshot_path, mem_file_path, source_rootfs_path) = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let snapshot = snapshots.get_mut(&snapshot_id).ok_or_else(|| AppError::NotFound(snapshot_id.clone()))?;
        check_no_live_fork(snapshot.forked_into.as_deref(), &snapshot_id, "forking")?;
        snapshot.forked_into = Some(new_id.clone());
        (snapshot.snapshot_path.clone(), snapshot.mem_file_path.clone(), snapshot.rootfs_path.clone())
    };

    // The fork gets its own disk, cloned BEFORE it resumes and handed to it as
    // `rootfs_override`. The snapshot records its rootfs by host path, so
    // without this the fork would write straight into the snapshot's disk and
    // the next fork or resume of the same snapshot would read a corrupted
    // filesystem (reproduced: ext4 `Bad message`). Fatal on failure: a shared
    // rootfs is silently unsafe, not merely degraded.
    let new_rootfs_path = crate::storage::rootfs_path(&new_id);
    let release_fork = |state: &Arc<AppState>| {
        if let Some(snapshot) = state.snapshots.lock().unwrap().get_mut(&snapshot_id) {
            snapshot.forked_into = None;
        }
    };
    if let Err(e) = spawn_blocking_in_current_span("fork rootfs clone task panicked", {
        let source_rootfs_path = source_rootfs_path.clone();
        let new_rootfs_path = new_rootfs_path.clone();
        let state = state.clone();
        move || crate::lifecycle::create::clone_rootfs(&source_rootfs_path, &new_rootfs_path, &state.metrics)
    })
    .await
    {
        release_fork(&state);
        return Err(AppError::from(e));
    }
    let mut rootfs_guard = crate::storage::OwnedFile::new(new_rootfs_path.clone());

    let (vm, tap) = match resume_vm(&state, snapshot_path, mem_file_path, Some(new_rootfs_path.clone())).await {
        Ok(resumed) => resumed,
        Err(e) => {
            release_fork(&state);
            return Err(AppError::from(e));
        }
    };
    rootfs_guard.keep();

    let (sandbox, egress, guest_ip, tap_device) = {
        let snapshots = state.snapshots.lock().unwrap();
        // Can't have been removed: delete/resume both refuse while
        // `forked_into` is set, and it's set to `new_id` for this call.
        let snapshot = snapshots.get(&snapshot_id).expect("reserved by this call above");
        let guest_ip = snapshot.network.addr.guest_ip;
        let tap_device = tap.device().to_string();
        let sandbox = Sandbox {
            id: new_id.clone(),
            vm,
            network: None,
            // The snapshot keeps the IP; the fork owns only the tap it is plugged into.
            fork_network: Some(ForkNetwork { tap, guest_ip }),
            rootfs_path: new_rootfs_path,
            attached_drives: snapshot.attached_drives.clone(),
            image_id: snapshot.image_id.clone(),
            // Jailer covers `Vm::boot` only — every resume/fork spawns
            // directly regardless of the original sandbox's jail state.
            jail_id: None,
            tags: snapshot.tags.clone(),
            created_at: SystemTime::now(),
            last_activity: std::sync::Mutex::new(std::time::Instant::now()),
            source_snapshot_id: Some(snapshot_id.clone()),
            // Fork and source snapshot deliberately carry the same name
            // at once — see `Sandbox::name`, `AppState::resolve_name`'s
            // live-wins priority.
            name: snapshot.name.clone(),
            pty_session_count: Default::default(),
            // Not a pool claim either — same as resume's construction above.
            source_pool_id: None,
            // Not recorded here (a fork is never snapshotted again);
            // `Snapshot::egress` is the copy applied to the fork's tap just
            // below, and `destroy_sandbox_by_id` removes the chain with the tap.
            egress: None,
            // No external resource to double-own here, unlike egress —
            // `env` is plain data, copied like resume does.
            env: snapshot.env.clone(),
            // Same value as `source_snapshot_id` for a fork specifically
            // (unlike resume, where the two diverge) — still its own
            // field, see `Sandbox::parent_snapshot_id`.
            parent_snapshot_id: Some(snapshot_id.clone()),
            // Carried straight over, not re-applied — see resume's
            // identical field above.
            mounts: snapshot.mounts.clone(),
            log_sessions: Default::default(),
        };
        (sandbox, snapshot.egress.clone(), guest_ip, tap_device)
    };
    state.sandboxes.lock().unwrap().insert(new_id.clone(), sandbox);

    // Same reasoning as resume's re-application: a failure here is a
    // loud warning, not fatal — the snapshot isn't lost (still there,
    // unconsumed), but tearing down a fresh fork over an iptables hiccup
    // is worse than a security warning for a best-effort layer.
    if let Some(policy) = &egress {
        if let Err(e) = sandkiln_vmm::egress::apply(guest_ip, &tap_device, state.network.uplink(), policy) {
            tracing::error!(sandbox_id = %new_id, error = %e, "failed to re-apply this sandbox's egress policy after forking — it is running WITHOUT its configured network restrictions enforced");
        }
    }


    Ok(new_id)
}

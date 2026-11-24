//! Resume a held snapshot into a new live sandbox.
//!
//! Owns `resume_snapshot_by_id` (consume the record, optionally retire the
//! checkpoint into time-travel history), `resume_vm` (the blocking
//! `Vm::resume`, also used by fork and history restore) and the filesystem
//! half of retiring (`retire_snapshot_files`). A failed resume puts the record
//! back so the caller can retry.

use super::check_no_live_fork;
use crate::http::error::AppError;
use crate::sandbox::Sandbox;
use crate::snapshots::held::Snapshot;
use crate::snapshots::retired::RetiredSnapshot;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use sandkiln_vmm::vm::{ResumeConfig, Vm};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use uuid::Uuid;

/// Removes a snapshot's `state.snap`/`mem.bin`/`meta.json` from
/// `dest_dir`'s parent (the hot-or-archived directory it actually lived
/// in), already moved out to `dest_dir` by `retire_snapshot_files` — what
/// remains is either an empty directory or pre-upgrade stale files.
fn cleanup_old_snapshot_dir(old_snapshot_dir: Option<&std::path::Path>) {
    if let Some(dir) = old_snapshot_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Retires `snapshot` into `crate::snapshots::retired` instead of
/// discarding it: moves `state.snap`/`mem.bin` into the retired directory and
/// records the checkpoint. The checkpoint keeps the snapshot's *original*
/// rootfs, which must stay pristine because its `state.snap` describes that
/// file as of the moment it was taken. The caller therefore resumes the new
/// sandbox onto a private clone (`resume_vm`'s `rootfs_override`) *before*
/// calling this.
///
/// Pure filesystem work, run on a blocking thread by the caller.
/// `old_snapshot_dir` is removed only
/// after both files are moved out of it; on any failure, whatever was
/// already created is cleaned up and `old_snapshot_dir` is left intact
/// so a caller falling back to the original destructive behavior still
/// finds a complete snapshot directory to remove.
fn retire_snapshot_files(snapshot: &Snapshot, old_snapshot_dir: Option<&std::path::Path>) -> io::Result<RetiredSnapshot> {
    let dest_dir = crate::snapshots::retired::retired_dir(&snapshot.id);
    let cleanup_partial = |extra: &std::path::Path| {
        let _ = std::fs::remove_file(extra);
        let _ = std::fs::remove_dir_all(&dest_dir);
    };

    std::fs::create_dir_all(&dest_dir)?;
    let new_state = crate::snapshots::held::state_path(&dest_dir);
    let new_mem = crate::snapshots::held::mem_path(&dest_dir);
    if let Err(e) = crate::snapshots::held::move_file(&snapshot.snapshot_path, &new_state) {
        cleanup_partial(&new_state);
        return Err(e);
    }
    // `state.snap` already moved. Rolling back could fail for the same reason
    // this does, so it stays in `dest_dir`: a future `reconcile()` sees a
    // directory missing `mem.bin` and skips it with a warning, like any other
    // crashed-mid-write case.
    crate::snapshots::held::move_file(&snapshot.mem_file_path, &new_mem)?;

    let retired = RetiredSnapshot {
        id: snapshot.id.clone(),
        source_sandbox_id: snapshot.source_sandbox_id.clone(),
        snapshot_path: new_state,
        mem_file_path: new_mem,
        rootfs_path: snapshot.rootfs_path.clone(),
        network: snapshot.network.config.clone(),
        host_octet: snapshot.network.host_octet(),
        attached_drives: snapshot.attached_drives.clone(),
        image_id: snapshot.image_id.clone(),
        tags: snapshot.tags.clone(),
        created_at: snapshot.created_at,
        retired_at: SystemTime::now(),
        name: snapshot.name.clone(),
        parent_snapshot_id: snapshot.parent_snapshot_id.clone(),
        egress: snapshot.egress.clone(),
        env: snapshot.env.clone(),
        mounts: snapshot.mounts.clone(),
    };
    if let Err(e) = retired.persist(&dest_dir) {
        // Best-effort: state.snap/mem.bin are already safely in place —
        // a missing/corrupt meta.json is `load_one`'s "incomplete
        // directory" case, same non-fatal treatment as elsewhere.
        tracing::warn!(
            snapshot_id = %snapshot.id, error = %e,
            "moved a retired checkpoint's files but failed to persist its metadata — \
             it will not reconcile after a daemon restart until this is fixed by hand"
        );
    }

    cleanup_old_snapshot_dir(old_snapshot_dir);
    Ok(retired)
}

/// Consumes a held snapshot's *record* and boots a new sandbox from it —
/// retired the moment this call starts (a concurrent resume of the same
/// id 404s rather than racing two VMs onto one rootfs file), refused
/// (409) while a fork is alive. A failed resume attempt puts the record
/// back so the caller can retry.
///
/// `retain_history` controls the *checkpoint data* once the resume has
/// succeeded (see `crate::snapshots::retired`, "time-travel restore"):
/// - `true` (default): checkpoint is *retired*, restorable later via
///   `POST /snapshots/history/:id/restore`; new sandbox gets a private
///   rootfs clone. A failure partway through retiring is a loud warning,
///   never fatal — the VM already resumed, so this degrades to the
///   original destructive behavior for this one resume rather than
///   losing the caller's live sandbox.
/// - `false`: original behavior — old files deleted, new sandbox reuses
///   the rootfs directly, no clone/retention overhead.
///
/// Shared by `resume_snapshot` (by id) and `get_or_create_sandbox` (by
/// name, always `retain_history: true`) — one place owns what resuming
/// means.
pub(crate) async fn resume_snapshot_by_id(
    state: Arc<AppState>,
    snapshot_id: String,
    retain_history: bool,
) -> Result<String, AppError> {
    let snapshot = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let existing = snapshots.get(&snapshot_id).ok_or_else(|| AppError::NotFound(snapshot_id.clone()))?;
        check_no_live_fork(existing.forked_into.as_deref(), &snapshot_id, "resuming")?;
        snapshots.remove(&snapshot_id).expect("just checked it exists")
    };

    let new_id = Uuid::new_v4().to_string();
    // Not a hardcoded `snapshot_dir(&snapshot_id)` — an archived
    // snapshot's files live under `Config::archive_dir` instead, and
    // cleaning the wrong directory would leak the real files forever.
    let old_snapshot_dir = snapshot.snapshot_path.parent().map(|p| p.to_path_buf());
    // Retaining history means the checkpoint must stay pristine, so the new
    // sandbox runs on its own clone, made BEFORE it resumes and handed over as
    // `rootfs_override` (the snapshot records its disk by host path, so a VM
    // resumed without an override writes into the checkpoint's own disk).
    // Without retention the snapshot is consumed and the VM uses its disk in
    // place. A clone failure degrades to that destructive mode rather than
    // failing the caller's resume.
    let mut retain_history = retain_history;
    let mut private_rootfs: Option<PathBuf> = None;
    if retain_history {
        let dest = crate::storage::rootfs_path(&new_id);
        let cloned = spawn_blocking_in_current_span("resume rootfs clone task panicked", {
            let source = snapshot.rootfs_path.clone();
            let dest = dest.clone();
            let state = state.clone();
            move || crate::lifecycle::create::clone_rootfs(&source, &dest, &state.metrics)
        })
        .await;
        match cloned {
            Ok(()) => private_rootfs = Some(dest),
            Err(e) => {
                tracing::warn!(
                    snapshot_id = %snapshot_id, error = %e,
                    "could not clone the rootfs to retain history for this resume; resuming destructively instead"
                );
                retain_history = false;
            }
        }
    }

    let result = resume_vm(&state, snapshot.snapshot_path.clone(), snapshot.mem_file_path.clone(), private_rootfs.clone()).await;

    let vm = match result {
        Ok(vm) => vm,
        Err(e) => {
            if let Some(clone) = &private_rootfs {
                let _ = std::fs::remove_file(clone);
            }
            state.snapshots.lock().unwrap().insert(snapshot_id, snapshot);
            return Err(AppError::from(e));
        }
    };

    // VM has resumed successfully — everything below decides what
    // happens to the checkpoint *data* and must never turn this success
    // into a failure response.
    let (snapshot, retire_result) = spawn_blocking_in_current_span("retire-checkpoint task panicked", {
        let old_snapshot_dir = old_snapshot_dir.clone();
        move || {
            let result = if retain_history {
                retire_snapshot_files(&snapshot, old_snapshot_dir.as_deref()).map(Some)
            } else {
                Ok(None)
            };
            (snapshot, result)
        }
    })
    .await;

    let rootfs_path = match (retire_result, private_rootfs) {
        (Ok(Some(retired)), Some(clone)) => {
            state.retired_snapshots.lock().unwrap().insert(retired.id.clone(), retired);
            crate::http::routes::snapshot_history::enforce_retention(&state).await;
            clone
        }
        // Not retaining: the snapshot was consumed, the VM runs on its disk.
        (Ok(None), _) => {
            cleanup_old_snapshot_dir(old_snapshot_dir.as_deref());
            snapshot.rootfs_path.clone()
        }
        // Retiring failed after the VM resumed on its private clone: no
        // restorable checkpoint exists for this resume, and the original disk
        // is now unreferenced, so remove it rather than leak it.
        (Err(e), Some(clone)) => {
            tracing::warn!(
                snapshot_id = %snapshot_id, error = %e,
                "failed to retain history for this resume — falling back to the original, fully \
                 destructive behavior for this one resume (no restorable checkpoint will exist for it)"
            );
            cleanup_old_snapshot_dir(old_snapshot_dir.as_deref());
            let _ = std::fs::remove_file(&snapshot.rootfs_path);
            clone
        }
        (Ok(Some(_)), None) | (Err(_), None) => unreachable!("history is only retained when a private rootfs clone exists"),
    };

    // Captured before `snapshot.network`/`snapshot.egress` move into the
    // `Sandbox` literal below.
    let egress = snapshot.egress.clone();
    let guest_ip = snapshot.network.config.guest_ip;
    let tap_device = snapshot.network.config.tap_device.clone();

    let sandbox = Sandbox {
        id: new_id.clone(),
        vm,
        network: Some(snapshot.network),
        rootfs_path,
        attached_drives: snapshot.attached_drives,
        image_id: snapshot.image_id,
        // `Vm::resume` always spawns directly — never jailed.
        jail_id: None,
        tags: snapshot.tags,
        created_at: SystemTime::now(),
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        source_snapshot_id: None,
        name: snapshot.name,
        pty_session_count: Default::default(),
        // A plain resume isn't a pool claim — only
        // `claim_from_pool` (which calls this, then overwrites this
        // field) ties a resume to a pool.
        source_pool_id: None,
        egress: egress.clone(),
        env: snapshot.env,
        // Unlike `source_snapshot_id` (deliberately `None` so this stays
        // snapshottable), lineage wants the real origin recorded — see
        // `Sandbox::parent_snapshot_id`.
        parent_snapshot_id: Some(snapshot_id.clone()),
        // Carried straight over, not re-applied — a mount is a live
        // guest FUSE process, already restored with the rest of the
        // snapshotted memory image.
        mounts: snapshot.mounts,
        log_sessions: Default::default(),
    };
    state.sandboxes.lock().unwrap().insert(new_id.clone(), sandbox);

    // Re-applied idempotently rather than assumed still installed —
    // correct whether the chain survived (daemon restart) or needs
    // recreating (host reboot wiped it). A failure here is a loud
    // warning, not fatal: this snapshot is already consumed (one-way),
    // so destroying the freshly resumed sandbox over an iptables hiccup
    // would be real data loss for what's a best-effort hardening layer.
    if let Some(policy) = &egress {
        if let Err(e) = sandkiln_vmm::egress::apply(guest_ip, &tap_device, state.network.uplink(), policy) {
            tracing::error!(sandbox_id = %new_id, error = %e, "failed to re-apply this sandbox's egress policy after resume — it is running WITHOUT its configured network restrictions enforced");
        }
    }

    Ok(new_id)
}

// `pub(crate)`: also used by `routes::snapshot_history`'s restore path —
// loading a VM from state.snap/mem.bin is identical whether those files
// belong to a live `Snapshot` or a dormant `RetiredSnapshot`.
///
/// `rootfs_override` is the disk the resumed VM will write to. Pass a private
/// clone (made *before* this call) for anything that must leave the snapshot
/// reusable: a fork, a restorable checkpoint, a history restore. `None` makes
/// the VM write into the snapshot's own disk, which is only correct when the
/// snapshot is being consumed. Without this, a resumed VM silently corrupts
/// the snapshot it came from (see `ResumeConfig::rootfs_override`).
pub(crate) async fn resume_vm(
    state: &Arc<AppState>,
    snapshot_path: PathBuf,
    mem_file_path: PathBuf,
    rootfs_override: Option<PathBuf>,
) -> std::io::Result<Vm> {
    let state = state.clone();
    spawn_blocking_in_current_span("resume task panicked", move || {
        Vm::resume(&ResumeConfig {
            firecracker_bin: state.config.firecracker_bin.clone(),
            snapshot_path,
            mem_file_path,
            rootfs_override,
        })
    })
    .await
}

//! Snapshot/resume/fork: save a running sandbox's full state (memory +
//! device state) to disk and stop it, then later boot a *new* sandbox
//! straight from that save point instead of a fresh rootfs + kernel boot.
//! See `ROADMAP.md`'s "Persistence and snapshotting" section for the shape
//! this is working toward.
//!
//! Kept in its own module/router rather than folded into `routes.rs` — it
//! only ever removes entries from `state.sandboxes` and adds them to
//! `state.snapshots`, so it doesn't need to touch the existing handlers
//! there at all.
//!
//! Two ways to boot from a snapshot:
//! - `POST /snapshots/:id/resume` **consumes** the snapshot: the record
//!   and its on-disk state/memory files are gone afterward, and the
//!   resulting sandbox owns the rootfs file and network lease (if any)
//!   outright, same as a freshly created one. This is the original,
//!   unchanged behavior, kept as the default for exactly that reason:
//!   nothing that already depends on "one resume, then it's gone" breaks.
//! - `POST /snapshots/:id/fork` does **not** consume it — the snapshot
//!   stays around, ready to be forked or resumed again later. This is the
//!   building block for the "VM forking" work described in `ROADMAP.md`:
//!   start a new sandbox from an exact prepared save point without paying
//!   setup cost again, more than once.
//!
//! Forking is intentionally **not** the same as true concurrent forking of
//! a running VM: `Vm::resume`'s `/snapshot/load` call reopens the *exact*
//! rootfs file path recorded in the snapshot's own serialized state (and,
//! if the source sandbox was networked, the exact tap device — the
//! guest's IP/MAC are baked into the snapshotted memory image itself, see
//! `sandkiln_vmm::vm::Vm::resume`'s doc comment). Firecracker has no
//! documented way to redirect a resumed VM's drive to a different backing
//! file at load time the way `network_overrides` can rename a tap device,
//! so two live descendants of one snapshot would mean two Firecracker
//! processes writing the *same* rootfs file concurrently — real
//! filesystem corruption — and, for a networked snapshot, two guests
//! presenting the identical boot-time IP/MAC on the shared bridge at
//! once — a real address collision neither guest is aware of, since
//! reassigning it would need in-guest cooperation this project's guest
//! agent doesn't have. `Snapshot::forked_into` is the lock that rules
//! both out: at most one live sandbox descended from a given snapshot may
//! exist at a time, whether it got there via `/fork` or (before it
//! consumed the snapshot) `/resume`. That still delivers the real,
//! useful part of forking — resuming the exact same prepared state
//! repeatedly, without ever losing the ability to go back to it — just
//! not simultaneous parallel branches from one snapshot. True concurrent
//! forking would need either a verified Firecracker mechanism to give
//! each fork an independent rootfs backing file, or a from-scratch
//! live-memory-clone approach instead of snapshot/resume; neither is
//! implemented here.

use crate::error::AppError;
use crate::sandbox::Sandbox;
use crate::snapshot::Snapshot;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use sandkiln_vmm::vm::{ResumeConfig, Vm};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sandboxes/:id/snapshot", post(snapshot_sandbox))
        .route("/snapshots", get(list_snapshots))
        .route("/snapshots/:id/resume", post(resume_snapshot))
        .route("/snapshots/:id/fork", post(fork_snapshot))
        .route("/snapshots/:id", delete(delete_snapshot))
}

#[derive(Serialize)]
pub struct SnapshotSandboxResponse {
    snapshot_id: String,
}

/// Pauses the sandbox's microVM, snapshots it to disk, and stops the VM
/// process — the sandbox stops existing as a live `Sandbox`, and a
/// `Snapshot` record takes its place. The network lease and rootfs image
/// aren't released/removed the way `stop_sandbox` does it: both are held
/// by the `Snapshot` so `resume_snapshot`/`fork_snapshot` can hand them
/// straight to the new sandbox.
#[tracing::instrument(skip(state))]
pub async fn snapshot_sandbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SnapshotSandboxResponse>, AppError> {
    // A forked descendant shares its rootfs file with the snapshot it came
    // from rather than owning a private copy (see the module doc comment)
    // — snapshotting it would produce a second `Snapshot` record pointing
    // at that same shared file, cascading the exact resume-time conflict
    // `forked_into` exists to prevent. Stop it and fork the original
    // snapshot again instead.
    {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        if let Some(source) = &sandbox.source_snapshot_id {
            return Err(AppError::Conflict(format!(
                "sandbox {id} was forked from snapshot {source} and shares its rootfs file — stop it and fork \
                 {source} again instead of snapshotting {id} directly"
            )));
        }
    }

    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
    let Sandbox { vm, network, rootfs_path, attached_drives, tags, .. } = sandbox;
    // Only a forked descendant (rejected above) ever has `network: None`.
    let network = network.expect("non-fork sandboxes always hold a network lease");

    let snapshot_id = Uuid::new_v4().to_string();
    let dir = snapshot_dir(&snapshot_id);
    let snapshot_path = dir.join("state.snap");
    let mem_file_path = dir.join("mem.bin");

    let result = tokio::task::spawn_blocking({
        let dir = dir.clone();
        let snapshot_path = snapshot_path.clone();
        let mem_file_path = mem_file_path.clone();
        move || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let outcome = vm.pause().and_then(|_| vm.snapshot(&mem_file_path, &snapshot_path));
            // Whether or not the snapshot succeeded, this VM is done —
            // a paused VM that failed to snapshot isn't something we can
            // hand back to the caller as still-running. Stop it either
            // way so the process/sockets are never leaked.
            let _ = vm.stop();
            outcome
        }
    })
    .await
    .expect("snapshot task panicked");

    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup_state = state.clone();
        tokio::task::spawn_blocking(move || {
            let _ = cleanup_state.network.release(network);
            let _ = std::fs::remove_file(&rootfs_path);
        })
        .await
        .expect("cleanup task panicked");
        return Err(AppError::from(e));
    }

    let snapshot = Snapshot {
        id: snapshot_id.clone(),
        source_sandbox_id: id,
        snapshot_path,
        mem_file_path,
        rootfs_path,
        network,
        attached_drives,
        tags,
        created_at: SystemTime::now(),
        forked_into: None,
    };
    state.snapshots.lock().unwrap().insert(snapshot_id.clone(), snapshot);

    Ok(Json(SnapshotSandboxResponse { snapshot_id }))
}

#[derive(Serialize)]
pub struct SnapshotSummary {
    id: String,
    source_sandbox_id: String,
    created_at_unix: u64,
    tags: HashMap<String, String>,
    /// Id of the live sandbox currently forked from this snapshot, if any
    /// — see `Snapshot::forked_into`. While set, `/fork`, `/resume`, and
    /// `DELETE` on this snapshot are all rejected with a 409.
    forked_into: Option<String>,
}

#[derive(Serialize)]
pub struct ListSnapshotsResponse {
    snapshots: Vec<SnapshotSummary>,
}

pub async fn list_snapshots(State(state): State<Arc<AppState>>) -> Json<ListSnapshotsResponse> {
    let snapshots = state
        .snapshots
        .lock()
        .unwrap()
        .values()
        .map(|s| SnapshotSummary {
            id: s.id.clone(),
            source_sandbox_id: s.source_sandbox_id.clone(),
            created_at_unix: s.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            tags: s.tags.clone(),
            forked_into: s.forked_into.clone(),
        })
        .collect();
    Json(ListSnapshotsResponse { snapshots })
}

#[derive(Serialize)]
pub struct ResumeSnapshotResponse {
    id: String,
}

/// Boots a brand-new sandbox by loading a snapshot instead of cloning the
/// base rootfs and booting fresh. This consumes the snapshot: on success
/// its record and on-disk state/memory files are removed (the resumed
/// sandbox now owns the rootfs and network lease, and can be snapshotted
/// again itself for a new save point), and on failure the record is put
/// back so the caller can retry rather than silently losing it. Removing
/// the record up front — before the resume attempt — also means a second
/// concurrent resume of the same snapshot 404s instead of racing two VMs
/// onto the same rootfs file. Refuses (409) while a fork of this snapshot
/// is still alive, for the same reason: see the module doc comment.
#[tracing::instrument(skip(state))]
pub async fn resume_snapshot(
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<String>,
) -> Result<Json<ResumeSnapshotResponse>, AppError> {
    let snapshot = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let existing = snapshots.get(&snapshot_id).ok_or_else(|| AppError::NotFound(snapshot_id.clone()))?;
        check_no_live_fork(existing.forked_into.as_deref(), &snapshot_id, "resuming")?;
        snapshots.remove(&snapshot_id).expect("just checked it exists")
    };

    let new_id = Uuid::new_v4().to_string();
    let result = resume_vm(&state, snapshot.snapshot_path.clone(), snapshot.mem_file_path.clone()).await;

    let vm = match result {
        Ok(vm) => vm,
        Err(e) => {
            state.snapshots.lock().unwrap().insert(snapshot_id, snapshot);
            return Err(AppError::from(e));
        }
    };

    let sandbox = Sandbox {
        id: new_id.clone(),
        vm,
        network: Some(snapshot.network),
        rootfs_path: snapshot.rootfs_path,
        attached_drives: snapshot.attached_drives,
        tags: snapshot.tags,
        created_at: SystemTime::now(),
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        source_snapshot_id: None,
    };
    state.sandboxes.lock().unwrap().insert(new_id.clone(), sandbox);

    // Only good for one resume — Firecracker doesn't need these files
    // again once the VM is loaded and running, and the resumed sandbox
    // itself can be snapshotted anew for a fresh save point.
    let _ = std::fs::remove_dir_all(snapshot_dir(&snapshot_id));

    Ok(Json(ResumeSnapshotResponse { id: new_id }))
}

#[derive(Serialize)]
pub struct ForkSnapshotResponse {
    id: String,
}

/// Boots a new sandbox from a snapshot *without* consuming it — the
/// record and its on-disk state/memory files are left exactly as they
/// were, so this snapshot can be forked (or finally resumed) again later.
/// The forked sandbox does not own the snapshot's rootfs file or network
/// lease (if any): both stay with the `Snapshot`, and this sandbox's own
/// teardown (`stop_sandbox_by_id`) must not touch either.
///
/// Rejected with 409 while an earlier fork of this snapshot is still
/// alive — see the module doc comment for why more than one live
/// descendant at a time isn't safe. The id is reserved for the new
/// sandbox up front (before the slow resume call) precisely so a second
/// concurrent `/fork` request sees the reservation and 409s immediately,
/// instead of racing another `Vm::resume` onto the same rootfs file.
#[tracing::instrument(skip(state))]
pub async fn fork_snapshot(
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<String>,
) -> Result<Json<ForkSnapshotResponse>, AppError> {
    let new_id = Uuid::new_v4().to_string();

    let (snapshot_path, mem_file_path) = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let snapshot = snapshots.get_mut(&snapshot_id).ok_or_else(|| AppError::NotFound(snapshot_id.clone()))?;
        check_no_live_fork(snapshot.forked_into.as_deref(), &snapshot_id, "forking")?;
        snapshot.forked_into = Some(new_id.clone());
        (snapshot.snapshot_path.clone(), snapshot.mem_file_path.clone())
    };

    let result = resume_vm(&state, snapshot_path, mem_file_path).await;

    let vm = match result {
        Ok(vm) => vm,
        Err(e) => {
            if let Some(snapshot) = state.snapshots.lock().unwrap().get_mut(&snapshot_id) {
                snapshot.forked_into = None;
            }
            return Err(AppError::from(e));
        }
    };

    let sandbox = {
        let snapshots = state.snapshots.lock().unwrap();
        // Can't have been removed: `delete_snapshot` and `resume_snapshot`
        // both refuse while `forked_into` is set, and it's set to
        // `new_id` for the duration of this call.
        let snapshot = snapshots.get(&snapshot_id).expect("reserved by this call above");
        Sandbox {
            id: new_id.clone(),
            vm,
            network: None,
            rootfs_path: snapshot.rootfs_path.clone(),
            attached_drives: snapshot.attached_drives.clone(),
            tags: snapshot.tags.clone(),
            created_at: SystemTime::now(),
            last_activity: std::sync::Mutex::new(std::time::Instant::now()),
            source_snapshot_id: Some(snapshot_id.clone()),
        }
    };
    state.sandboxes.lock().unwrap().insert(new_id.clone(), sandbox);

    Ok(Json(ForkSnapshotResponse { id: new_id }))
}

async fn resume_vm(state: &Arc<AppState>, snapshot_path: PathBuf, mem_file_path: PathBuf) -> std::io::Result<Vm> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        Vm::resume(&ResumeConfig { firecracker_bin: state.config.firecracker_bin.clone(), snapshot_path, mem_file_path })
    })
    .await
    .expect("resume task panicked")
}

/// Pure decision behind every guard in this file: an operation that needs
/// exclusive use of a snapshot's shared resources (forking, consuming
/// resume, or deletion) may proceed only while no earlier fork of it is
/// still alive. Pulled out of the handlers so it's testable without axum
/// or the daemon's mutex-guarded state.
fn check_no_live_fork(forked_into: Option<&str>, snapshot_id: &str, action: &str) -> Result<(), AppError> {
    match forked_into {
        Some(holder) => Err(AppError::Conflict(format!(
            "snapshot {snapshot_id} has a live fork ({holder}) — stop it before {action} this snapshot"
        ))),
        None => Ok(()),
    }
}

/// Deletes a snapshot outright: releases its held network lease, removes
/// its rootfs copy and its state/memory files. For a snapshot the caller
/// has decided they'll never resume. Refuses (409) while a fork of it is
/// still alive, for the same reason `resume_snapshot` does.
#[tracing::instrument(skip(state))]
pub async fn delete_snapshot(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let snapshot = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let existing = snapshots.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        check_no_live_fork(existing.forked_into.as_deref(), &id, "deleting")?;
        snapshots.remove(&id).expect("just checked it exists")
    };

    tokio::task::spawn_blocking({
        let state = state.clone();
        move || {
            let _ = state.network.release(snapshot.network);
            let _ = std::fs::remove_file(&snapshot.rootfs_path);
            let _ = std::fs::remove_dir_all(snapshot_dir(&snapshot.id));
        }
    })
    .await
    .expect("delete task panicked");

    Ok(StatusCode::NO_CONTENT)
}

/// Where one snapshot's state + memory files live: a dedicated directory
/// per snapshot under the daemon's temp dir, alongside the loose
/// `sandkiln-rootfs-*.ext4` files `create_sandbox` writes there — mirrors
/// that same "OS temp dir, daemon-prefixed" convention rather than
/// inventing a new storage location.
fn snapshot_dir(snapshot_id: &str) -> PathBuf {
    std::env::temp_dir().join("sandkiln-snapshots").join(snapshot_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_live_fork_allows_the_operation() {
        assert!(check_no_live_fork(None, "snap-1", "forking").is_ok());
    }

    #[test]
    fn a_live_fork_blocks_forking_with_a_clear_message() {
        let err = check_no_live_fork(Some("sbx-9"), "snap-1", "forking").unwrap_err();
        let AppError::Conflict(message) = err else { panic!("expected Conflict, got a different AppError variant") };
        assert!(message.contains("snap-1"), "message was: {message}");
        assert!(message.contains("sbx-9"), "message was: {message}");
        assert!(message.contains("forking"), "message was: {message}");
    }

    #[test]
    fn a_live_fork_blocks_resuming_and_deleting_too() {
        assert!(check_no_live_fork(Some("sbx-9"), "snap-1", "resuming").is_err());
        assert!(check_no_live_fork(Some("sbx-9"), "snap-1", "deleting").is_err());
    }
}

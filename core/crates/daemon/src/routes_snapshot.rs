//! Snapshot/resume: save a running sandbox's full state (memory + device
//! state) to disk and stop it, then later boot a *new* sandbox straight
//! from that save point instead of a fresh rootfs + kernel boot. See
//! `ROADMAP.md`'s "Persistence and snapshotting" section for the shape
//! this is working toward.
//!
//! Kept in its own module/router rather than folded into `routes.rs` — it
//! only ever removes entries from `state.sandboxes` and adds them to
//! `state.snapshots`, so it doesn't need to touch the existing handlers
//! there at all.

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
/// by the `Snapshot` so `resume_snapshot` can hand them straight to the
/// new sandbox.
#[tracing::instrument(skip(state))]
pub async fn snapshot_sandbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SnapshotSandboxResponse>, AppError> {
    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
    let Sandbox { vm, network, rootfs_path, tags, .. } = sandbox;

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
        tags,
        created_at: SystemTime::now(),
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
/// onto the same rootfs file.
#[tracing::instrument(skip(state))]
pub async fn resume_snapshot(
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<String>,
) -> Result<Json<ResumeSnapshotResponse>, AppError> {
    let snapshot =
        state.snapshots.lock().unwrap().remove(&snapshot_id).ok_or_else(|| AppError::NotFound(snapshot_id.clone()))?;

    let new_id = Uuid::new_v4().to_string();
    let result = tokio::task::spawn_blocking({
        let state = state.clone();
        let snapshot_path = snapshot.snapshot_path.clone();
        let mem_file_path = snapshot.mem_file_path.clone();
        move || -> std::io::Result<Vm> {
            Vm::resume(&ResumeConfig { firecracker_bin: state.config.firecracker_bin.clone(), snapshot_path, mem_file_path })
        }
    })
    .await
    .expect("resume task panicked");

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
        network: snapshot.network,
        rootfs_path: snapshot.rootfs_path,
        tags: snapshot.tags,
        created_at: SystemTime::now(),
    };
    state.sandboxes.lock().unwrap().insert(new_id.clone(), sandbox);

    // Only good for one resume — Firecracker doesn't need these files
    // again once the VM is loaded and running, and the resumed sandbox
    // itself can be snapshotted anew for a fresh save point.
    let _ = std::fs::remove_dir_all(snapshot_dir(&snapshot_id));

    Ok(Json(ResumeSnapshotResponse { id: new_id }))
}

/// Deletes a snapshot outright: releases its held network lease, removes
/// its rootfs copy and its state/memory files. For a snapshot the caller
/// has decided they'll never resume.
#[tracing::instrument(skip(state))]
pub async fn delete_snapshot(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    let snapshot = state.snapshots.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;

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

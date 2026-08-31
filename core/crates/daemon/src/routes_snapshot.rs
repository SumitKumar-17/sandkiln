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
use crate::snapshot::{snapshot_dir, Snapshot};
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::extract::{Path, Query, State};
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

/// Why a sandbox can't be snapshotted right now — the two structural
/// (not transient) reasons `check_snapshottable` can refuse. Split out
/// from `SnapshotStopError` so each caller of `snapshot_and_stop` can
/// react differently: `snapshot_sandbox` (a direct, explicit ask) always
/// turns either into an error, while `routes_sandbox::stop_sandbox_by_id`
/// (an implicit "preserve by default" ask) treats `ForkedFrom` as
/// harmless — see that function's doc comment for why — and only
/// `Jailed` as a real conflict there too.
pub(crate) enum SnapshotBlocked {
    /// Firecracker's own snapshot/device state bakes in the in-jail paths
    /// (e.g. "/rootfs.ext4") a jailed sandbox's chroot used, and
    /// `Vm::resume` only ever spawns directly — resuming such a snapshot
    /// would try to open those paths against the *host's* real root
    /// filesystem and fail (or, worse, coincidentally resolve to an
    /// unrelated file). See `sandkiln_vmm::jailer`'s module doc comment.
    Jailed,
    /// This sandbox was forked from the named snapshot and shares its
    /// rootfs file rather than owning a private copy (see the module doc
    /// comment) — snapshotting it would produce a second `Snapshot`
    /// record pointing at that same shared file, cascading the exact
    /// resume-time conflict `forked_into` exists to prevent.
    ForkedFrom(String),
}

/// Pure precondition behind `snapshot_and_stop`: can this sandbox be
/// snapshotted at all? Pulled out for direct unit testing, mirroring
/// `check_no_live_fork` below.
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
    /// Default mapping used by `snapshot_sandbox` (`POST .../snapshot`) —
    /// a direct, explicit ask that has no fallback, so both `Blocked`
    /// reasons become real errors. `routes_sandbox::stop_sandbox_by_id`
    /// does **not** use this: it treats `ForkedFrom` as a non-error (see
    /// its doc comment) and only maps `Jailed` to its own
    /// `?keep=false`-mentioning message, so it matches on
    /// `SnapshotStopError` directly instead of going through this.
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

/// Pauses a sandbox's microVM, snapshots it to disk, and stops the VM
/// process — the sandbox stops existing as a live `Sandbox`, and a
/// `Snapshot` record (with the same `name`, if any — see `Sandbox::name`)
/// takes its place in `AppState`. The network lease and rootfs image
/// aren't released/removed the way a full destroy does it: both are held
/// by the `Snapshot` so `resume_snapshot_by_id`/`fork_snapshot` can hand
/// them straight to the new sandbox.
///
/// Shared by `snapshot_sandbox` (`POST /sandboxes/:id/snapshot`, explicit),
/// `routes_sandbox::stop_sandbox_by_id` (`DELETE /sandboxes/:id`'s default
/// "preserve by stopping" behavior), and `idle_reaper`'s auto-suspend pass
/// — the actual pause/snapshot/stop mechanics live in exactly one place so
/// none of the three can drift apart on what "snapshot this sandbox" means.
pub(crate) async fn snapshot_and_stop(state: Arc<AppState>, id: String) -> Result<String, SnapshotStopError> {
    // Checked before removing the sandbox from the map, so a rejected
    // request leaves it exactly as it was (still running, still listed)
    // rather than needing to be put back.
    {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or(SnapshotStopError::NotFound)?;
        check_snapshottable(sandbox.vm.is_jailed(), sandbox.source_snapshot_id.as_deref())
            .map_err(SnapshotStopError::Blocked)?;
    }

    let sandbox = state.sandboxes.lock().unwrap().remove(&id).ok_or(SnapshotStopError::NotFound)?;
    let Sandbox { vm, network, rootfs_path, attached_drives, image_id, tags, name, .. } = sandbox;
    // Only a forked descendant (rejected above) ever has `network: None`.
    let network = network.expect("non-fork sandboxes always hold a network lease");

    let snapshot_id = Uuid::new_v4().to_string();
    let dir = snapshot_dir(&snapshot_id);
    let snapshot_path = dir.join("state.snap");
    let mem_file_path = dir.join("mem.bin");

    let result = spawn_blocking_in_current_span("snapshot task panicked", {
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
    .await;

    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup_state = state.clone();
        spawn_blocking_in_current_span("cleanup task panicked", move || {
            let _ = cleanup_state.network.release(network);
            let _ = std::fs::remove_file(&rootfs_path);
        })
        .await;
        return Err(SnapshotStopError::Io(e));
    }

    let snapshot = Snapshot {
        id: snapshot_id.clone(),
        source_sandbox_id: id,
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
    };

    // Persist metadata before this snapshot is visible in `AppState` at
    // all: `state.snap`/`mem.bin` already exist on disk at this point
    // (the earlier spawn_blocking succeeded), so if metadata fails to
    // write, the only correct move is the same one a failed snapshot
    // itself gets — tear the whole thing down and return an error — not
    // to keep a `Snapshot` alive in memory whose durability contract is
    // already broken.
    let (snapshot, persist_result) = tokio::task::spawn_blocking(move || {
        let persist_result = snapshot.persist();
        (snapshot, persist_result)
    })
    .await
    .expect("persist snapshot metadata task panicked");

    if let Err(e) = persist_result {
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup_state = state.clone();
        tokio::task::spawn_blocking(move || {
            let _ = cleanup_state.network.release(snapshot.network);
            let _ = std::fs::remove_file(&snapshot.rootfs_path);
        })
        .await
        .expect("cleanup task panicked");
        return Err(SnapshotStopError::Io(e));
    }

    state.snapshots.lock().unwrap().insert(snapshot_id.clone(), snapshot);

    Ok(snapshot_id)
}

/// Pauses the sandbox's microVM, snapshots it to disk, and stops the VM
/// process — the sandbox stops existing as a live `Sandbox`, and a
/// `Snapshot` record takes its place. Thin HTTP wrapper around
/// `snapshot_and_stop`; see that function's doc comment for the mechanics.
#[tracing::instrument(skip(state))]
pub async fn snapshot_sandbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SnapshotSandboxResponse>, AppError> {
    let snapshot_id = snapshot_and_stop(state, id.clone()).await.map_err(|e| match e {
        SnapshotStopError::NotFound => AppError::NotFound(id),
        other => AppError::from(other),
    })?;
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
    /// Carried over from the sandbox this was taken from — see
    /// `Sandbox::name`'s doc comment. `GET /sandboxes/by-name/:name` and
    /// `POST /sandboxes/get-or-create` are how a caller finds this
    /// snapshot again by it.
    name: Option<String>,
}

#[derive(Serialize)]
pub struct ListSnapshotsResponse {
    snapshots: Vec<SnapshotSummary>,
}

/// Optional `?source_sandbox_id=<id>` narrows the listing to snapshots
/// taken from that one original sandbox id — the mechanism a caller uses
/// to go from "the sandbox id I had" to "the snapshot it became" after an
/// auto-suspend (or a manual `POST /sandboxes/:id/snapshot`) makes that
/// sandbox disappear from `GET /sandboxes`. At most one snapshot can ever
/// match, since a sandbox id is retired the moment it's snapshotted and
/// never reused, but this stays a filter on the plural listing (mirroring
/// `list_sandboxes`'s `?tag.<key>=` filtering) rather than a separate
/// single-result endpoint, since "no match" (still running, or genuinely
/// gone) and "one match" both need to be representable without a 404
/// forcing every poller to treat "not found yet" as an error to retry
/// around.
pub async fn list_snapshots(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<ListSnapshotsResponse> {
    let source_sandbox_id_filter = query.get("source_sandbox_id").map(String::as_str);

    let snapshots = state
        .snapshots
        .lock()
        .unwrap()
        .values()
        .filter(|s| source_sandbox_id_filter.is_none_or(|wanted| s.source_sandbox_id == wanted))
        .map(|s| SnapshotSummary {
            id: s.id.clone(),
            source_sandbox_id: s.source_sandbox_id.clone(),
            created_at_unix: s.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            tags: s.tags.clone(),
            forked_into: s.forked_into.clone(),
            name: s.name.clone(),
        })
        .collect();
    Json(ListSnapshotsResponse { snapshots })
}

#[derive(Serialize)]
pub struct ResumeSnapshotResponse {
    id: String,
}

/// Consumes a held snapshot and boots a new sandbox from it: on success
/// the snapshot's record and on-disk state/memory files are removed (the
/// resumed sandbox now owns the rootfs and network lease, and can be
/// snapshotted again itself for a new save point), and on failure the
/// record is put back so the caller can retry rather than silently
/// losing it. Removing the record up front — before the resume attempt —
/// is also what makes a second concurrent resume of the same snapshot
/// 404 instead of racing two VMs onto the same rootfs file. Refuses
/// (409) while a fork of this snapshot is still alive, for the same
/// reason: see the module doc comment.
///
/// Shared by `resume_snapshot` (`POST /snapshots/:id/resume`, by id) and
/// `routes_sandbox_name::get_or_create_sandbox` (by name, once resolved
/// to a snapshot id) — same reasoning as `snapshot_and_stop` above: one
/// place owns "what resuming a snapshot means."
pub(crate) async fn resume_snapshot_by_id(state: Arc<AppState>, snapshot_id: String) -> Result<String, AppError> {
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
        image_id: snapshot.image_id,
        // `Vm::resume` always spawns directly (see its doc comment) —
        // never jailed, so there's no uid/gid allocation to track here.
        jail_id: None,
        tags: snapshot.tags,
        created_at: SystemTime::now(),
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        source_snapshot_id: None,
        name: snapshot.name,
    };
    state.sandboxes.lock().unwrap().insert(new_id.clone(), sandbox);

    // Only good for one resume — Firecracker doesn't need these files
    // again once the VM is loaded and running, and the resumed sandbox
    // itself can be snapshotted anew for a fresh save point.
    let _ = std::fs::remove_dir_all(snapshot_dir(&snapshot_id));

    Ok(new_id)
}

/// Boots a brand-new sandbox by loading a snapshot instead of cloning the
/// base rootfs and booting fresh. Thin HTTP wrapper around
/// `resume_snapshot_by_id`; see that function's doc comment.
#[tracing::instrument(skip(state))]
pub async fn resume_snapshot(
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<String>,
) -> Result<Json<ResumeSnapshotResponse>, AppError> {
    let id = resume_snapshot_by_id(state, snapshot_id).await?;
    Ok(Json(ResumeSnapshotResponse { id }))
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
            image_id: snapshot.image_id.clone(),
            // Jailer support covers `Vm::boot` only — every resume/fork
            // (this path) always spawns directly, regardless of whether
            // the original sandbox was jailed. See `jailer.rs`'s module
            // doc comment and `snapshot_sandbox`'s own jailed-sandbox
            // rejection above.
            jail_id: None,
            tags: snapshot.tags.clone(),
            created_at: SystemTime::now(),
            last_activity: std::sync::Mutex::new(std::time::Instant::now()),
            source_snapshot_id: Some(snapshot_id.clone()),
            // Both this live fork and the snapshot it came from carry the
            // same name at once, deliberately — see `Sandbox::name`'s doc
            // comment and `AppState::resolve_name`'s live-wins priority.
            name: snapshot.name.clone(),
        }
    };
    state.sandboxes.lock().unwrap().insert(new_id.clone(), sandbox);

    Ok(Json(ForkSnapshotResponse { id: new_id }))
}

async fn resume_vm(state: &Arc<AppState>, snapshot_path: PathBuf, mem_file_path: PathBuf) -> std::io::Result<Vm> {
    let state = state.clone();
    spawn_blocking_in_current_span("resume task panicked", move || {
        Vm::resume(&ResumeConfig { firecracker_bin: state.config.firecracker_bin.clone(), snapshot_path, mem_file_path })
    })
    .await
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

    spawn_blocking_in_current_span("delete task panicked", {
        let state = state.clone();
        move || {
            let _ = state.network.release(snapshot.network);
            let _ = std::fs::remove_file(&snapshot.rootfs_path);
            let _ = std::fs::remove_dir_all(snapshot_dir(&snapshot.id));
        }
    })
    .await;

    Ok(StatusCode::NO_CONTENT)
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
        // `source_snapshot_id`, and `Vm::resume`/`fork` never jail), but
        // pins a deterministic precedence rather than leaving it
        // unspecified if that ever changed.
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

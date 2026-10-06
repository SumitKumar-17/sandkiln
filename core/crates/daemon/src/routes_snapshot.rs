//! Snapshot/resume/fork: save a running sandbox's full state (memory +
//! device state) to disk and stop it, then boot a *new* sandbox straight
//! from that save point instead of a fresh rootfs+kernel boot. See
//! `ROADMAP.md`'s "Persistence and snapshotting" section for the full
//! design. Own module/router since it only touches `state.sandboxes`/
//! `state.snapshots`, nothing in `routes.rs`.
//!
//! Two ways to boot from a snapshot:
//! - `POST /snapshots/:id/resume` **consumes the `Snapshot` record** —
//!   removed from `state.snapshots`, the new sandbox owns the network
//!   lease outright. By default does **not** consume the underlying
//!   checkpoint data (see `crate::snapshot_history`, "time-travel
//!   restore"): the resumed sandbox gets a fresh rootfs clone
//!   (`routes_sandbox::clone_rootfs`), and the original files move into
//!   a *retired*, still-restorable checkpoint. `?retain_history=false`
//!   opts back into the original fully-destructive behavior.
//! - `POST /snapshots/:id/fork` does **not** consume it — stays around
//!   to be forked/resumed again, the building block for repeated boots
//!   from one exact prepared state.
//!
//! **Forking is not true concurrent VM forking.** `Vm::resume`'s
//! `/snapshot/load` reopens the *exact* rootfs path and (if networked)
//! tap device baked into the snapshot's own state — guest IP/MAC are
//! frozen into the memory image itself (`Vm::resume`'s doc comment).
//! Firecracker has no way to redirect a resumed VM's drive to a
//! different backing file, so two live descendants of one snapshot would
//! mean two processes writing the same rootfs file (corruption) and two
//! guests presenting the same IP/MAC on the bridge (a collision neither
//! guest can resolve, since reassigning needs in-guest cooperation this
//! project's agent doesn't have). `Snapshot::forked_into` rules both out:
//! at most one live descendant of a snapshot at a time, via `/fork` or
//! (pre-consumption) `/resume`. True concurrent forking would need
//! either a verified per-fork rootfs mechanism from Firecracker or a
//! from-scratch live-memory-clone approach — neither exists here.
//!
//! **A fork does get its own private rootfs clone**, though — fixes a
//! real *sequential* corruption bug found while building time-travel
//! restore (same class: before this, a fork shared its source's rootfs
//! file directly, with only the tap/lease exclusive via `forked_into`).
//! Fork, mutate the shared file, stop the fork, then resume the original
//! directly: `Vm::resume` loads memory state describing the pre-fork
//! rootfs, against a file now carrying the fork's mutations.
//! `routes_sandbox::clone_rootfs` closes this the same way history
//! retention does — the fork gets an independent copy, the source
//! snapshot's file is never touched by anything but its own eventual
//! resume/fork.

use crate::error::AppError;
use crate::sandbox::Sandbox;
use crate::snapshot::{snapshot_dir, Snapshot};
use crate::snapshot_history::RetiredSnapshot;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use sandkiln_vmm::vm::{ResumeConfig, Vm};
use serde::Serialize;
use std::collections::HashMap;
use std::io;
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

/// Why a sandbox can't be snapshotted — the two structural (not
/// transient) reasons `check_snapshottable` refuses. Split from
/// `SnapshotStopError` so each caller reacts differently:
/// `snapshot_sandbox` turns either into an error, while
/// `routes_sandbox::stop_sandbox_by_id` treats `ForkedFrom` as harmless
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
/// any) takes its place. Network lease and rootfs aren't
/// released/removed (unlike a full destroy): both move to the
/// `Snapshot` for `resume_snapshot_by_id`/`fork_snapshot` to hand on.
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
    // (see `crate::pool`).
    if let Some(pool_id) = &sandbox.source_pool_id {
        if let Some(pool) = state.pools.lock().unwrap().get_mut(pool_id) {
            pool.record_release();
        }
    }
    let Sandbox { vm, network, rootfs_path, attached_drives, image_id, tags, name, egress, env, parent_snapshot_id, mounts, .. } =
        sandbox;
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
            // Either way this VM is done — a paused VM that failed to
            // snapshot can't be handed back as still-running.
            let _ = vm.stop();
            outcome
        }
    })
    .await;

    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&dir);
        let cleanup_state = state.clone();
        spawn_blocking_in_current_span("cleanup task panicked", move || {
            // Lease is fully released here (not left dormant), so the
            // egress chain goes with it. No-op if there was never one.
            sandkiln_vmm::egress::remove(network.config.guest_ip, &network.config.tap_device, cleanup_state.network.uplink());
            let _ = cleanup_state.network.release(network);
            let _ = std::fs::remove_file(&rootfs_path);
        })
        .await;
        return Err(SnapshotStopError::Io(e));
    }

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
            let _ = cleanup_state.network.release(snapshot.network);
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

/// Thin HTTP wrapper around `snapshot_and_stop` — see that function's
/// doc comment for the mechanics.
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
    /// Live sandbox currently forked from this snapshot, if any — see
    /// `Snapshot::forked_into`. While set, `/fork`/`/resume`/`DELETE`
    /// all 409.
    forked_into: Option<String>,
    /// Carried over from the source sandbox — see `Sandbox::name`.
    /// `GET /sandboxes/by-name/:name` / `POST /sandboxes/get-or-create`
    /// find this snapshot again by it.
    name: Option<String>,
    /// When moved to `Config::archive_dir` — `null` means still hot.
    archived_at_unix: Option<u64>,
    /// Snapshot this one was forked/resumed from, if any — see
    /// `Snapshot::parent_snapshot_id`. `?parent_snapshot_id=<id>` on this
    /// same listing answers the reverse direction; together they walk a
    /// full lineage tree one hop at a time.
    parent_snapshot_id: Option<String>,
}

#[derive(Serialize)]
pub struct ListSnapshotsResponse {
    snapshots: Vec<SnapshotSummary>,
}

/// `?source_sandbox_id=<id>` narrows to the snapshot taken from that
/// sandbox — how a caller goes from "the sandbox id I had" to "the
/// snapshot it became" after auto-suspend removes it from
/// `GET /sandboxes`. At most one can ever match (a sandbox id is retired
/// the moment it's snapshotted), but this stays a filter on the plural
/// listing rather than a single-result endpoint, so "no match" (still
/// running, or genuinely gone) doesn't force a 404 a poller has to treat
/// as an error to retry around.
///
/// `?parent_snapshot_id=<id>` is the reverse: "what was ever
/// forked/resumed from this one." Unlike `source_sandbox_id`, more than
/// one can match over time (`forked_into` only limits *live*
/// descendants, not how many have ever existed) — a real multi-result
/// filter. Combined with each summary's own `parent_snapshot_id`, a
/// caller walks a full lineage tree one hop (one request) at a time —
/// deliberately narrow rather than a tree-shaped endpoint, since a
/// deleted intermediate snapshot breaks the chain either way.
pub async fn list_snapshots(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<ListSnapshotsResponse> {
    let source_sandbox_id_filter = query.get("source_sandbox_id").map(String::as_str);
    let parent_snapshot_id_filter = query.get("parent_snapshot_id").map(String::as_str);

    let snapshots = state
        .snapshots
        .lock()
        .unwrap()
        .values()
        .filter(|s| source_sandbox_id_filter.is_none_or(|wanted| s.source_sandbox_id == wanted))
        .filter(|s| parent_snapshot_id_filter.is_none_or(|wanted| s.parent_snapshot_id.as_deref() == Some(wanted)))
        .map(|s| SnapshotSummary {
            id: s.id.clone(),
            source_sandbox_id: s.source_sandbox_id.clone(),
            created_at_unix: s.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            tags: s.tags.clone(),
            forked_into: s.forked_into.clone(),
            name: s.name.clone(),
            archived_at_unix: s.archived_at.map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()),
            parent_snapshot_id: s.parent_snapshot_id.clone(),
        })
        .collect();
    Json(ListSnapshotsResponse { snapshots })
}

#[derive(Serialize)]
pub struct ResumeSnapshotResponse {
    id: String,
}

/// Removes a snapshot's `state.snap`/`mem.bin`/`meta.json` from
/// `dest_dir`'s parent (the hot-or-archived directory it actually lived
/// in), already moved out to `dest_dir` by `retire_snapshot_files` — what
/// remains is either an empty directory or pre-upgrade stale files.
fn cleanup_old_snapshot_dir(old_snapshot_dir: Option<&std::path::Path>) {
    if let Some(dir) = old_snapshot_dir {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Retires `snapshot` into `crate::snapshot_history` instead of
/// discarding it, returning the path the *new* sandbox should use as its
/// rootfs — a fresh private clone, never the original file (continued
/// use of the new sandbox would otherwise retroactively corrupt a
/// checkpoint whose `state.snap` still describes the shared file as of
/// the moment it was taken).
///
/// Pure filesystem/process work (clone shells out to `cp`), run on a
/// blocking thread by the caller. `old_snapshot_dir` is removed only
/// after both files are moved out of it; on any failure, whatever was
/// already created is cleaned up and `old_snapshot_dir` is left intact
/// so a caller falling back to the original destructive behavior still
/// finds a complete snapshot directory to remove.
fn retire_snapshot_files(
    new_id: &str,
    snapshot: &Snapshot,
    old_snapshot_dir: Option<&std::path::Path>,
) -> io::Result<(PathBuf, RetiredSnapshot)> {
    let new_rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{new_id}.ext4"));
    crate::routes_sandbox::clone_rootfs(&snapshot.rootfs_path, &new_rootfs_path)?;

    let dest_dir = crate::snapshot_history::retired_dir(&snapshot.id);
    let cleanup_partial = |extra: &std::path::Path| {
        let _ = std::fs::remove_file(&new_rootfs_path);
        let _ = std::fs::remove_file(extra);
        let _ = std::fs::remove_dir_all(&dest_dir);
    };

    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        let _ = std::fs::remove_file(&new_rootfs_path);
        return Err(e);
    }
    let new_state = crate::snapshot::state_path(&dest_dir);
    let new_mem = crate::snapshot::mem_path(&dest_dir);
    if let Err(e) = crate::snapshot::move_file(&snapshot.snapshot_path, &new_state) {
        cleanup_partial(&new_state);
        return Err(e);
    }
    if let Err(e) = crate::snapshot::move_file(&snapshot.mem_file_path, &new_mem) {
        // `state.snap` already moved — rolling back could fail for the
        // same reason this branch was hit. Leaving it in `dest_dir` is
        // safe: a future `reconcile()` sees a directory missing
        // `mem.bin` and skips it with a warning, same as any other
        // crashed-mid-write case.
        let _ = std::fs::remove_file(&new_rootfs_path);
        return Err(e);
    }

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
    Ok((new_rootfs_path, retired))
}

/// Consumes a held snapshot's *record* and boots a new sandbox from it —
/// retired the moment this call starts (a concurrent resume of the same
/// id 404s rather than racing two VMs onto one rootfs file), refused
/// (409) while a fork is alive. A failed resume attempt puts the record
/// back so the caller can retry.
///
/// `retain_history` controls the *checkpoint data* once the resume has
/// succeeded (see `crate::snapshot_history`, "time-travel restore"):
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
    let result = resume_vm(&state, snapshot.snapshot_path.clone(), snapshot.mem_file_path.clone()).await;

    let vm = match result {
        Ok(vm) => vm,
        Err(e) => {
            state.snapshots.lock().unwrap().insert(snapshot_id, snapshot);
            return Err(AppError::from(e));
        }
    };

    // VM has resumed successfully — everything below decides what
    // happens to the checkpoint *data* and must never turn this success
    // into a failure response.
    let (snapshot, retire_result) = spawn_blocking_in_current_span("retire-checkpoint task panicked", {
        let new_id = new_id.clone();
        let old_snapshot_dir = old_snapshot_dir.clone();
        move || {
            let result = if retain_history {
                retire_snapshot_files(&new_id, &snapshot, old_snapshot_dir.as_deref()).map(Some)
            } else {
                Ok(None)
            };
            (snapshot, result)
        }
    })
    .await;

    let rootfs_path = match retire_result {
        Ok(Some((new_rootfs_path, retired))) => {
            state.retired_snapshots.lock().unwrap().insert(retired.id.clone(), retired);
            new_rootfs_path
        }
        Ok(None) => {
            cleanup_old_snapshot_dir(old_snapshot_dir.as_deref());
            snapshot.rootfs_path.clone()
        }
        Err(e) => {
            tracing::warn!(
                snapshot_id = %snapshot_id, error = %e,
                "failed to retain history for this resume — falling back to the original, fully \
                 destructive behavior for this one resume (no restorable checkpoint will exist for it)"
            );
            cleanup_old_snapshot_dir(old_snapshot_dir.as_deref());
            snapshot.rootfs_path.clone()
        }
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

/// Mirrors `routes_sandbox::parse_keep`'s shape exactly.
fn parse_retain_history(params: &HashMap<String, String>) -> Result<bool, String> {
    match params.get("retain_history").map(String::as_str) {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(format!("invalid 'retain_history' query parameter '{other}': expected 'true' or 'false'")),
    }
}

/// Thin HTTP wrapper around `resume_snapshot_by_id` — see that
/// function's doc comment for what `?retain_history=` (default `true`)
/// controls.
#[tracing::instrument(skip(state))]
pub async fn resume_snapshot(
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<ResumeSnapshotResponse>, AppError> {
    let retain_history = parse_retain_history(&params).map_err(AppError::BadRequest)?;
    let id = resume_snapshot_by_id(state, snapshot_id, retain_history).await?;
    Ok(Json(ResumeSnapshotResponse { id }))
}

#[derive(Serialize)]
pub struct ForkSnapshotResponse {
    id: String,
}

/// Boots a new sandbox from a snapshot *without* consuming it — record
/// and files untouched, so it can be forked/resumed again later. The
/// forked sandbox doesn't own the snapshot's rootfs/lease (both stay
/// with the `Snapshot`); `stop_sandbox_by_id` must never touch either.
///
/// 409 while an earlier fork is still alive (see module doc comment).
/// The new id is reserved up front, before the slow resume call, so a
/// second concurrent `/fork` sees the reservation and 409s immediately
/// instead of racing another `Vm::resume` onto the same rootfs.
#[tracing::instrument(skip(state))]
pub async fn fork_snapshot(
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<String>,
) -> Result<Json<ForkSnapshotResponse>, AppError> {
    let new_id = Uuid::new_v4().to_string();

    let (snapshot_path, mem_file_path, source_rootfs_path) = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let snapshot = snapshots.get_mut(&snapshot_id).ok_or_else(|| AppError::NotFound(snapshot_id.clone()))?;
        check_no_live_fork(snapshot.forked_into.as_deref(), &snapshot_id, "forking")?;
        snapshot.forked_into = Some(new_id.clone());
        (snapshot.snapshot_path.clone(), snapshot.mem_file_path.clone(), snapshot.rootfs_path.clone())
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

    // Own private rootfs clone rather than sharing the source's file —
    // see module doc comment for the sequential-corruption bug this
    // closes. Fatal on failure (unlike resume's best-effort retention):
    // a shared rootfs would be silently unsafe, not merely degraded.
    let new_rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{new_id}.ext4"));
    if let Err(e) = spawn_blocking_in_current_span("fork rootfs clone task panicked", {
        let source_rootfs_path = source_rootfs_path.clone();
        let new_rootfs_path = new_rootfs_path.clone();
        move || crate::routes_sandbox::clone_rootfs(&source_rootfs_path, &new_rootfs_path)
    })
    .await
    {
        let _ = vm.stop();
        if let Some(snapshot) = state.snapshots.lock().unwrap().get_mut(&snapshot_id) {
            snapshot.forked_into = None;
        }
        return Err(AppError::from(e));
    }

    let (sandbox, egress, guest_ip, tap_device) = {
        let snapshots = state.snapshots.lock().unwrap();
        // Can't have been removed: delete/resume both refuse while
        // `forked_into` is set, and it's set to `new_id` for this call.
        let snapshot = snapshots.get(&snapshot_id).expect("reserved by this call above");
        let sandbox = Sandbox {
            id: new_id.clone(),
            vm,
            network: None,
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
            // Not owned here either (`network: None` convention) — the
            // iptables chain is tied to the lease, which the *snapshot*
            // still owns for a fork. `Snapshot::egress` is the copy
            // actually (re-)applied just below.
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
        (sandbox, snapshot.egress.clone(), snapshot.network.config.guest_ip, snapshot.network.config.tap_device.clone())
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

    Ok(Json(ForkSnapshotResponse { id: new_id }))
}

// `pub(crate)`: also used by `routes_snapshot_history`'s restore path —
// loading a VM from state.snap/mem.bin is identical whether those files
// belong to a live `Snapshot` or a dormant `RetiredSnapshot`.
pub(crate) async fn resume_vm(state: &Arc<AppState>, snapshot_path: PathBuf, mem_file_path: PathBuf) -> std::io::Result<Vm> {
    let state = state.clone();
    spawn_blocking_in_current_span("resume task panicked", move || {
        Vm::resume(&ResumeConfig { firecracker_bin: state.config.firecracker_bin.clone(), snapshot_path, mem_file_path })
    })
    .await
}

/// Pure decision behind every guard here: an operation needing exclusive
/// use of a snapshot's shared resources (fork, consuming resume,
/// deletion) may proceed only while no earlier fork is still alive.
/// Pulled out for direct unit testing.
fn check_no_live_fork(forked_into: Option<&str>, snapshot_id: &str, action: &str) -> Result<(), AppError> {
    match forked_into {
        Some(holder) => Err(AppError::Conflict(format!(
            "snapshot {snapshot_id} has a live fork ({holder}) — stop it before {action} this snapshot"
        ))),
        None => Ok(()),
    }
}

/// Deletes a snapshot outright: releases its lease, removes its rootfs
/// copy and state/memory files. 409 while a fork is alive, same reason
/// as resume.
#[tracing::instrument(skip(state))]
pub async fn delete_snapshot(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    delete_snapshot_by_id(state, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The actual teardown, shared by `delete_snapshot` and `crate::pool`'s
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

/// Every way `archive_snapshot_by_id` can fail.
pub(crate) enum ArchiveError {
    NotFound,
    /// A live fork exists — same exclusion as resume/fork/delete, since
    /// its `Vm::resume` reopens this snapshot's *current* paths.
    Forked,
    Io(std::io::Error),
}

/// Moves an already-held, unforked snapshot's files to `archive_root`
/// via `crate::snapshot::move_snapshot_files`. Only called from
/// `idle_reaper`'s archive pass today — no on-demand archive route yet
/// (deliberately deferred, same precedent as `crate::pool`'s narrower
/// first slice).
///
/// Removed from `AppState::snapshots` for the duration of the move (same
/// reasoning as delete/resume: nothing else should touch it mid-move)
/// and **always** reinserted after, success or failure — a failed
/// archive must never lose track of it. See
/// `crate::snapshot::move_snapshot_files` for why a failure can leave a
/// genuinely mixed set of old/new paths, reinserted as-is for later
/// inspection rather than guessed at.
pub(crate) async fn archive_snapshot_by_id(state: Arc<AppState>, id: String, archive_root: PathBuf) -> Result<(), ArchiveError> {
    let snapshot = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let existing = snapshots.get(&id).ok_or(ArchiveError::NotFound)?;
        if existing.forked_into.is_some() {
            return Err(ArchiveError::Forked);
        }
        snapshots.remove(&id).expect("just checked it exists")
    };

    // Captured before the move mutates `snapshot.snapshot_path` — once
    // archived, this old dir has only a stale `meta.json` left, which
    // would otherwise make a future restart's `reconcile()` log a
    // false-alarm warning for a healthy, successfully-archived snapshot.
    let old_dir = snapshot.snapshot_path.parent().map(|p| p.to_path_buf());

    let dest_dir = crate::snapshot::archive_snapshot_dir(&archive_root, &id);
    let (snapshot, result) = spawn_blocking_in_current_span("archive snapshot task panicked", move || {
        let mut snapshot = snapshot;
        let result = crate::snapshot::move_snapshot_files(&mut snapshot, &dest_dir).and_then(|()| {
            snapshot.archived_at = Some(std::time::SystemTime::now());
            snapshot.persist(&dest_dir)
        });
        if result.is_ok() {
            if let Some(old_dir) = old_dir {
                let _ = std::fs::remove_dir_all(old_dir);
            }
        }
        (snapshot, result)
    })
    .await;

    state.snapshots.lock().unwrap().insert(id, snapshot);
    result.map_err(ArchiveError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_retain_history_defaults_to_true_when_absent() {
        assert_eq!(parse_retain_history(&HashMap::new()), Ok(true));
    }

    #[test]
    fn parse_retain_history_accepts_explicit_true_and_false() {
        assert_eq!(parse_retain_history(&HashMap::from([("retain_history".to_string(), "true".to_string())])), Ok(true));
        assert_eq!(parse_retain_history(&HashMap::from([("retain_history".to_string(), "false".to_string())])), Ok(false));
    }

    #[test]
    fn parse_retain_history_rejects_anything_else() {
        let err = parse_retain_history(&HashMap::from([("retain_history".to_string(), "yes".to_string())])).unwrap_err();
        assert!(err.contains("yes"), "message was: {err}");
    }

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

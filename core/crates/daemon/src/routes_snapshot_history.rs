//! `GET /snapshots/history` and `POST /snapshots/history/:id/restore` —
//! the caller-facing half of "time-travel restore." See
//! `crate::snapshot_history`'s module doc comment for the full design:
//! why a retired checkpoint holds no live network lease, what makes
//! retiring safe at all (the rootfs clone), and why this is sequential
//! ("go back to any earlier point") rather than branching (two
//! checkpoints from one lineage live at once — not attempted here, see
//! `routes_snapshot.rs`'s own module doc comment on why that's a
//! genuinely different, harder problem).

use crate::error::AppError;
use crate::sandbox::Sandbox;
use crate::snapshot_history::RetiredSnapshot;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/snapshots/history", get(list_snapshot_history))
        .route("/snapshots/history/:id/restore", post(restore_snapshot_history))
        .route("/snapshots/history/:id", delete(delete_snapshot_history))
}

#[derive(Serialize)]
pub struct RetiredSnapshotSummary {
    id: String,
    source_sandbox_id: String,
    created_at_unix: u64,
    retired_at_unix: u64,
    tags: HashMap<String, String>,
    name: Option<String>,
    /// See `crate::snapshot_history::RetiredSnapshot::parent_snapshot_id`.
    /// Combined with `?parent_snapshot_id=<id>` on this same listing (the
    /// reverse direction), a caller can walk a lineage's full retired
    /// history exactly the way `routes_snapshot::list_snapshots` already
    /// lets one walk *live* lineage — same filter shape, deliberately.
    parent_snapshot_id: Option<String>,
}

#[derive(Serialize)]
pub struct ListSnapshotHistoryResponse {
    checkpoints: Vec<RetiredSnapshotSummary>,
}

/// Same two filters, same reasoning, as `routes_snapshot::list_snapshots`
/// — `?source_sandbox_id=<id>` and `?parent_snapshot_id=<id>` narrow the
/// listing rather than needing a separate single-result endpoint.
pub async fn list_snapshot_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<ListSnapshotHistoryResponse> {
    let source_sandbox_id_filter = query.get("source_sandbox_id").map(String::as_str);
    let parent_snapshot_id_filter = query.get("parent_snapshot_id").map(String::as_str);

    let checkpoints = state
        .retired_snapshots
        .lock()
        .unwrap()
        .values()
        .filter(|r| source_sandbox_id_filter.is_none_or(|wanted| r.source_sandbox_id == wanted))
        .filter(|r| parent_snapshot_id_filter.is_none_or(|wanted| r.parent_snapshot_id.as_deref() == Some(wanted)))
        .map(|r| RetiredSnapshotSummary {
            id: r.id.clone(),
            source_sandbox_id: r.source_sandbox_id.clone(),
            created_at_unix: r.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            retired_at_unix: r.retired_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            tags: r.tags.clone(),
            name: r.name.clone(),
            parent_snapshot_id: r.parent_snapshot_id.clone(),
        })
        .collect();
    Json(ListSnapshotHistoryResponse { checkpoints })
}

#[derive(Serialize)]
pub struct RestoreSnapshotHistoryResponse {
    id: String,
}

/// RAII pairing for `AppState::try_reserve_pending_tap_restore`/
/// `release_pending_tap_restore` — releases the claim on drop, covering
/// every exit path (success, an early `?` return, or a panic unwind).
/// Mirrors `routes_sandbox::PendingImageBootGuard`'s exact shape.
struct PendingTapRestoreGuard {
    state: Arc<AppState>,
    tap_device: String,
}

impl Drop for PendingTapRestoreGuard {
    fn drop(&mut self) {
        self.state.release_pending_tap_restore(&self.tap_device);
    }
}

/// Restores a retired checkpoint into a brand-new live sandbox — see the
/// module doc comment. Thin HTTP wrapper; see
/// `restore_snapshot_history_by_id` for the actual mechanics.
#[tracing::instrument(skip(state))]
pub async fn restore_snapshot_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<RestoreSnapshotHistoryResponse>, AppError> {
    let new_id = restore_snapshot_history_by_id(state, id).await?;
    Ok(Json(RestoreSnapshotHistoryResponse { id: new_id }))
}

/// Does **not** consume the checkpoint being restored — it stays exactly
/// as it was in `AppState::retired_snapshots` afterward, restorable again
/// later as many times as wanted (see the module doc comment: this is
/// "rewind," not "pop"). The resulting sandbox owns everything outright —
/// its own freshly reserved network lease (`NetworkManager::reserve`,
/// same call `snapshot::reconcile` uses at startup, guarded here by the
/// conflict checks below since a restore, unlike startup reconciliation,
/// can race real, already-live use of this exact tap device) and its own
/// private rootfs clone (never the checkpoint's original file, so
/// whatever this sandbox does next can never retroactively corrupt the
/// checkpoint's own future restorability) — so unlike a fork, it stays
/// snapshottable afterward: `source_snapshot_id` is `None`, exactly
/// mirroring a plain resume's convention, not fork's. Only
/// `parent_snapshot_id` records where it actually came from.
pub(crate) async fn restore_snapshot_history_by_id(state: Arc<AppState>, id: String) -> Result<String, AppError> {
    let retired: RetiredSnapshot =
        state.retired_snapshots.lock().unwrap().get(&id).cloned().ok_or_else(|| AppError::NotFound(id.clone()))?;

    if let Some(holder) = state.tap_device_holder(&retired.network.tap_device) {
        return Err(AppError::Conflict(format!(
            "checkpoint {id}'s network identity is currently in use by {holder} — stop or destroy it first"
        )));
    }
    // Atomic claim-or-refuse: closes the race two *different* retired
    // checkpoints in the same lineage (sharing this identical tap device)
    // could otherwise hit if both are restored at once — see
    // `AppState::pending_tap_restores`'s own doc comment. The check just
    // above gives a more specific error when the conflict is a live
    // sandbox or held snapshot instead; this one covers the sibling-
    // restore case specifically.
    if !state.try_reserve_pending_tap_restore(&retired.network.tap_device) {
        return Err(AppError::Conflict(format!(
            "another restore is already in progress for checkpoint {id}'s network identity — try again shortly"
        )));
    }
    let _tap_guard = PendingTapRestoreGuard { state: state.clone(), tap_device: retired.network.tap_device.clone() };

    // A checkpoint's name is inert while it just sits in history (see the
    // module doc comment: destroying a name's "current" holder outright
    // frees the name for reuse, on purpose, rather than retired
    // checkpoints holding it hostage forever) — so restoring one has to
    // re-check the name is still free *now*, under the same per-name lock
    // every other name-claiming path uses, rather than trust whatever was
    // true back when this checkpoint was retired.
    let _name_guard = match &retired.name {
        Some(name) => {
            if let Some(holder) = state.name_holder(name) {
                return Err(AppError::Conflict(format!(
                    "checkpoint {id}'s name '{name}' is currently claimed by {holder} — stop or destroy it first"
                )));
            }
            Some(state.lock_name(name).await)
        }
        None => None,
    };

    let new_id = Uuid::new_v4().to_string();
    let new_rootfs_path = std::env::temp_dir().join(format!("sandkiln-rootfs-{new_id}.ext4"));
    spawn_blocking_in_current_span("restore rootfs clone task panicked", {
        let source = retired.rootfs_path.clone();
        let dest = new_rootfs_path.clone();
        move || crate::routes_sandbox::clone_rootfs(&source, &dest)
    })
    .await
    .map_err(AppError::from)?;

    let result = crate::routes_snapshot::resume_vm(&state, retired.snapshot_path.clone(), retired.mem_file_path.clone()).await;
    let vm = match result {
        Ok(vm) => vm,
        Err(e) => {
            let _ = std::fs::remove_file(&new_rootfs_path);
            return Err(AppError::from(e));
        }
    };

    let lease = state.network.reserve(retired.network.clone(), retired.host_octet);
    let guest_ip = lease.config.guest_ip;
    let tap_device = lease.config.tap_device.clone();

    let sandbox = Sandbox {
        id: new_id.clone(),
        vm,
        network: Some(lease),
        rootfs_path: new_rootfs_path,
        attached_drives: retired.attached_drives.clone(),
        image_id: retired.image_id.clone(),
        // `Vm::resume` always spawns directly — never jailed, so there's
        // no uid/gid allocation to track here, same as a plain resume.
        jail_id: None,
        tags: retired.tags.clone(),
        created_at: SystemTime::now(),
        last_activity: std::sync::Mutex::new(std::time::Instant::now()),
        source_snapshot_id: None,
        name: retired.name.clone(),
        pty_session_count: Default::default(),
        source_pool_id: None,
        egress: retired.egress.clone(),
        parent_snapshot_id: Some(retired.id.clone()),
        // Carried straight over, not re-applied -- a mount is a live
        // guest-side FUSE process, restored along with everything else
        // in the checkpoint's memory image. See
        // `crate::routes_mounts`'s module doc comment.
        mounts: retired.mounts.clone(),
        log_sessions: Default::default(),
    };
    state.sandboxes.lock().unwrap().insert(new_id.clone(), sandbox);

    // Re-applied (idempotently), not assumed still installed — same
    // reasoning as `resume_snapshot_by_id`'s own re-application: a
    // warning, not fatal, since this checkpoint isn't consumed by the
    // restore (unlike a plain resume, there's no "irreversible" framing
    // here at all — the caller can just restore it again), but tearing
    // down a freshly restored sandbox over an iptables hiccup is still a
    // worse outcome than a security warning for what remains a
    // best-effort hardening layer.
    if let Some(policy) = &retired.egress {
        if let Err(e) = sandkiln_vmm::egress::apply(guest_ip, &tap_device, state.network.uplink(), policy) {
            tracing::error!(
                sandbox_id = %new_id, error = %e,
                "failed to re-apply this sandbox's egress policy after restoring a retired checkpoint — \
                 it is running WITHOUT its configured network restrictions enforced"
            );
        }
    }

    Ok(new_id)
}

/// Deletes a retired checkpoint outright: its `state.snap`/`mem.bin` and
/// its own private rootfs file. Not a deferred nice-to-have — retiring is
/// the new default behavior of every `POST /snapshots/:id/resume`, so
/// without a way to reclaim this, disk usage grows unbounded purely from
/// normal use. **Found live, the hard way**, building this exact
/// feature: repeated manual verification during development filled a
/// 468GB disk to 100% via nothing but ordinary resumes, each one
/// retiring a checkpoint (a full guest-memory dump, comparable to the
/// sandbox's configured RAM) that nothing could ever clean up.
///
/// Removed from `AppState::retired_snapshots` *before* the files are
/// touched, mirroring `routes_snapshot::resume_snapshot_by_id`'s own
/// remove-before-act ordering — a concurrent restore's own lookup either
/// sees this checkpoint or doesn't, atomically, rather than racing a
/// half-deleted one.
#[tracing::instrument(skip(state))]
pub async fn delete_snapshot_history(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let retired = state.retired_snapshots.lock().unwrap().remove(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
    spawn_blocking_in_current_span("delete retired checkpoint task panicked", move || {
        let _ = std::fs::remove_file(&retired.rootfs_path);
        if let Some(dir) = retired.snapshot_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    })
    .await;
    Ok(StatusCode::NO_CONTENT)
}

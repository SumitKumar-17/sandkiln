//! Snapshot/resume/fork: save a running sandbox's full state (memory +
//! device state) to disk and stop it, then boot a *new* sandbox straight
//! from that save point instead of a fresh rootfs+kernel boot. See
//! `docs/design-notes/snapshots.md` for the full
//! design. Own module/router since it only touches `state.sandboxes`/
//! `state.snapshots`, nothing in `routes.rs`.
//!
//! Two ways to boot from a snapshot:
//! - `POST /snapshots/:id/resume` **consumes the `Snapshot` record** —
//!   removed from `state.snapshots`, the new sandbox owns the network
//!   lease outright. By default does **not** consume the underlying
//!   checkpoint data (see `crate::snapshots::retired`, "time-travel
//!   restore"): the resumed sandbox gets a fresh rootfs clone
//!   (`lifecycle::create::clone_rootfs`), and the original files move into
//!   a *retired*, still-restorable checkpoint. `?retain_history=false`
//!   opts back into the original fully-destructive behavior.
//! - `POST /snapshots/:id/fork` does **not** consume it — stays around
//!   to be forked/resumed again, the building block for repeated boots
//!   from one exact prepared state.
//!
//! **Forking is not true concurrent VM forking.** `Vm::resume`'s
//! `/snapshot/load` reopens the tap device baked into the snapshot's own
//! state, and guest IP/MAC are frozen into the memory image itself, so two live
//! descendants of one snapshot would present the same IP/MAC on the bridge (a
//! collision neither guest can resolve without in-guest cooperation this
//! project's agent does not have). `Snapshot::forked_into` rules that out: at
//! most one live descendant of a snapshot at a time, via `/fork` or
//! (pre-consumption) `/resume`.
//!
//! **Disks are different: every fork gets its own.** A snapshot records its
//! rootfs by host path, so a VM resumed without a drive override writes straight
//! into the snapshot's own disk, and the next resume reads a filesystem that no
//! longer matches the memory it was captured with (reproduced as ext4
//! `Bad message`). A fork, a retain-history resume and a history restore
//! therefore clone the disk BEFORE resuming and pass the clone as
//! `ResumeConfig::rootfs_override` (Firecracker `PATCH /drives/rootfs` after a
//! paused load); the snapshot's original disk is never written. Only a consuming
//! resume, which retires the snapshot, uses its disk in place. Regression
//! coverage: `scripts/integration-tests/26-snapshot-disk-isolation.sh`.

use crate::http::error::AppError;
use crate::snapshots::ops::delete::delete_snapshot_by_id;
use crate::snapshots::ops::fork::fork_snapshot_by_id;
use crate::snapshots::ops::resume::resume_snapshot_by_id;
use crate::snapshots::ops::take::{snapshot_and_stop, SnapshotStopError};
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

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

/// Mirrors `routes::sandbox::parse_keep`'s shape exactly.
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


/// Thin HTTP wrapper around `fork_snapshot_by_id` — see that function's
/// doc comment for the 409-while-forked rule and the clone-before-resume
/// disk isolation.
#[tracing::instrument(skip(state))]
pub async fn fork_snapshot(
    State(state): State<Arc<AppState>>,
    Path(snapshot_id): Path<String>,
) -> Result<Json<ForkSnapshotResponse>, AppError> {
    let id = fork_snapshot_by_id(state, snapshot_id).await?;
    Ok(Json(ForkSnapshotResponse { id }))
}

/// Deletes a snapshot outright: releases its lease, removes its rootfs
/// copy and state/memory files. 409 while a fork is alive, same reason
/// as resume.
#[tracing::instrument(skip(state))]
pub async fn delete_snapshot(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<StatusCode, AppError> {
    delete_snapshot_by_id(state, id).await?;
    Ok(StatusCode::NO_CONTENT)
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
}

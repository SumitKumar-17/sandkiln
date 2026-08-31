//! HTTP handlers for persistent drives: attachable filesystem storage
//! that outlives any single sandbox and can be reattached to a new one.
//! Kept separate from `routes.rs` so this work lands as its own diff.
//!
//! A drive has no "detach" endpoint of its own — detaching happens
//! implicitly when the sandbox it's attached to is stopped
//! (`DELETE /sandboxes/:id`), which drops the sandbox (and its
//! `attached_drives` list) from `AppState::sandboxes` without touching
//! the drive's backing file. `DELETE /drives/:id` is for permanently
//! destroying a drive, and refuses to do so while it's still attached.

use crate::error::AppError;
use crate::state::{describe_drive_holders, AppState};
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// A drive to attach to a sandbox at creation time — accepted as part of
/// `POST /sandboxes`'s `drives` field, mirroring how `tags` is accepted
/// there.
#[derive(Deserialize)]
pub struct DriveAttachment {
    pub id: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Deserialize)]
pub struct CreateDriveRequest {
    size_mib: u64,
}

#[derive(Serialize)]
pub struct DriveHolderSummary {
    /// `"sandbox <id>"` or `"snapshot <id>"` — see `AppState::drive_holders`.
    holder: String,
    read_only: bool,
}

#[derive(Serialize)]
pub struct DriveSummary {
    id: String,
    size_mib: u64,
    created_at_unix: u64,
    /// Everything currently holding this drive — empty if nothing does.
    /// More than one entry means it's attached read-only to multiple
    /// sandboxes/snapshots at once (see `can_attach_read_only`); a drive
    /// can't be deleted while this is non-empty, regardless of whether
    /// every entry is read-only.
    attached_to: Vec<DriveHolderSummary>,
}

#[tracing::instrument(skip(state, body), fields(size_mib = body.size_mib))]
pub async fn create_drive(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateDriveRequest>,
) -> Result<Json<DriveSummary>, AppError> {
    let id = Uuid::new_v4().to_string();
    let size_mib = body.size_mib;

    spawn_blocking_in_current_span("create drive task panicked", {
        let state = state.clone();
        let id = id.clone();
        move || state.drives.create(&id, size_mib)
    })
    .await
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidInput => AppError::BadRequest(e.to_string()),
        _ => AppError::Internal(e),
    })?;

    Ok(Json(DriveSummary { id, size_mib, created_at_unix: now_unix(), attached_to: Vec::new() }))
}

#[derive(Serialize)]
pub struct ListDrivesResponse {
    drives: Vec<DriveSummary>,
}

pub async fn list_drives(State(state): State<Arc<AppState>>) -> Result<Json<ListDrivesResponse>, AppError> {
    let infos = spawn_blocking_in_current_span("list drives task panicked", {
        let state = state.clone();
        move || state.drives.list()
    })
    .await
    .map_err(AppError::Internal)?;

    let drives = infos
        .into_iter()
        .map(|info| {
            let attached_to = state
                .drive_holders(&info.id)
                .into_iter()
                .map(|h| DriveHolderSummary { holder: h.holder, read_only: h.read_only })
                .collect();
            DriveSummary {
                id: info.id,
                size_mib: info.size_bytes / (1024 * 1024),
                created_at_unix: info.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
                attached_to,
            }
        })
        .collect();

    Ok(Json(ListDrivesResponse { drives }))
}

#[tracing::instrument(skip(state))]
pub async fn delete_drive(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    let holders = state.drive_holders(&id);
    if !holders.is_empty() {
        return Err(AppError::Conflict(format!(
            "drive {id} is attached to {} — release it before deleting",
            describe_drive_holders(&holders)
        )));
    }

    spawn_blocking_in_current_span("delete drive task panicked", {
        let state = state.clone();
        let id = id.clone();
        move || state.drives.delete(&id)
    })
    .await
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => AppError::DriveNotFound(id.clone()),
        _ => AppError::Internal(e),
    })?;

    Ok(StatusCode::NO_CONTENT)
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

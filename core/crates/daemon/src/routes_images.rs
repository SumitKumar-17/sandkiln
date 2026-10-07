//! HTTP handlers for registered images: named, daemon-tracked ext4 rootfs
//! files a `POST /sandboxes` request can boot from instead of the
//! `SANDKILN_BASE_ROOTFS` default. Own file for the same reason
//! `routes_drives.rs` is.
//!
//! `POST /images` takes a host filesystem path, not a file upload —
//! accepting an arbitrary multi-gigabyte upload over HTTP is a distinct,
//! larger problem than registering a file that already exists. Converting
//! an OCI/Docker image into a bootable rootfs is out of scope entirely.
//!
//! **The daemon can never verify a registered image has the guest agent
//! baked in** — that needs loop-mounting as root
//! (`scripts/preflight-check.sh --root-checks`), and this daemon
//! deliberately has no root. Every response says so via
//! `ImageSummary::guest_agent_verified`/`verification_hint` rather than
//! implying boot-readiness — the common failure mode is an image that
//! boots fine but never answers `exec` because the agent was never
//! injected.

use crate::error::AppError;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Guidance repeated on every `ImageSummary` — see the module doc comment
/// for why the daemon can never fill in `guest_agent_verified: true`
/// itself.
const VERIFICATION_HINT: &str = "the daemon cannot verify the guest agent is baked into this image (that needs \
     loop-mounting it as root, which this unprivileged daemon does not have) — run \
     'scripts/preflight-check.sh --root-checks --rootfs-image <path>' against the source file before relying on \
     it, or a sandbox booted from it may boot but never respond to exec";

#[derive(Deserialize)]
pub struct CreateImageRequest {
    /// Caller-chosen, daemon-unique name, e.g. `"python-3.12-custom"`.
    /// Required (unlike a drive's server-generated id) — an image is a
    /// stable, memorable identity reused across many creates, not a
    /// one-off handle. Validated by `ImageStore`: no path separators,
    /// leading dots, or other unsafe filename characters (`400`).
    id: String,
    /// Absolute path, on the host filesystem the daemon process itself
    /// runs on, to an already-built ext4 rootfs image. Copied (not
    /// referenced in place) into the daemon's managed images directory —
    /// see `sandkiln_vmm::image::ImageStore::register`.
    path: String,
}

#[derive(Serialize)]
pub struct ImageSummary {
    id: String,
    size_mib: u64,
    created_at_unix: u64,
    /// What currently holds this image, if anything — `"sandbox <id>"`,
    /// `"snapshot <id>"`, or `"a sandbox currently being created"` for one
    /// still mid-boot (see `AppState::image_holder`). An image can't be
    /// deleted while this is set.
    in_use_by: Option<String>,
    /// Always `false` — see the module doc comment and `VERIFICATION_HINT`.
    guest_agent_verified: bool,
    verification_hint: String,
}

#[tracing::instrument(skip(state, body), fields(id = %body.id))]
pub async fn create_image(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateImageRequest>,
) -> Result<Json<ImageSummary>, AppError> {
    let id = body.id;
    let source = PathBuf::from(&body.path);

    let (size_bytes, created_at) = spawn_blocking_in_current_span("register image task panicked", {
        let state = state.clone();
        let id = id.clone();
        move || -> std::io::Result<(u64, SystemTime)> {
            let path = state.images.register(&id, &source)?;
            let metadata = std::fs::metadata(&path)?;
            Ok((metadata.len(), metadata.created().unwrap_or(SystemTime::UNIX_EPOCH)))
        }
    })
    .await
    .map_err(map_image_io_error)?;

    tracing::warn!(image_id = %id, "{VERIFICATION_HINT}");

    Ok(Json(ImageSummary {
        id,
        size_mib: size_bytes / (1024 * 1024),
        created_at_unix: created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
        in_use_by: None,
        guest_agent_verified: false,
        verification_hint: VERIFICATION_HINT.to_string(),
    }))
}

#[derive(Serialize)]
pub struct ListImagesResponse {
    images: Vec<ImageSummary>,
}

pub async fn list_images(State(state): State<Arc<AppState>>) -> Result<Json<ListImagesResponse>, AppError> {
    let infos = spawn_blocking_in_current_span("list images task panicked", {
        let state = state.clone();
        move || state.images.list()
    })
    .await
    .map_err(AppError::Internal)?;

    let images = infos
        .into_iter()
        .map(|info| {
            let in_use_by = state.image_holder(&info.id);
            ImageSummary {
                id: info.id,
                size_mib: info.size_bytes / (1024 * 1024),
                created_at_unix: info.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
                in_use_by,
                guest_agent_verified: false,
                verification_hint: VERIFICATION_HINT.to_string(),
            }
        })
        .collect();

    Ok(Json(ListImagesResponse { images }))
}

/// Refuses while any live sandbox or held snapshot still references this
/// image (`AppState::image_holder`, which also covers a boot currently in
/// flight from it — see `pending_image_boots`'s doc comment on `AppState`)
/// — the same "can't delete what's in use" pattern `routes_drives::delete_drive`
/// establishes via `AppState::drive_holder`.
#[tracing::instrument(skip(state))]
pub async fn delete_image(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    if let Some(holder) = state.image_holder(&id) {
        return Err(AppError::Conflict(format!("image {id} is in use by {holder} — stop or release it before deleting")));
    }

    spawn_blocking_in_current_span("delete image task panicked", {
        let state = state.clone();
        let id = id.clone();
        move || state.images.delete(&id)
    })
    .await
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => AppError::ImageNotFound(id.clone()),
        std::io::ErrorKind::InvalidInput => AppError::BadRequest(e.to_string()),
        _ => AppError::Internal(e),
    })?;

    Ok(StatusCode::NO_CONTENT)
}

/// Shared `io::Error` → `AppError` mapping for `ImageStore::register`'s
/// failure modes — pulled out since `create_image` is the only caller but
/// the match arms are easier to read named than inline.
fn map_image_io_error(e: std::io::Error) -> AppError {
    match e.kind() {
        std::io::ErrorKind::InvalidInput => AppError::BadRequest(e.to_string()),
        std::io::ErrorKind::AlreadyExists => AppError::Conflict(e.to_string()),
        // `register` also uses `NotFound`-shaped errors for "the source
        // path doesn't exist" — that's a bad request (the caller gave a
        // path that isn't there), not a 404 against this API's own
        // resources (`AppError::ImageNotFound` is reserved for "no image
        // with this id is registered", a different failure).
        std::io::ErrorKind::NotFound => AppError::BadRequest(e.to_string()),
        _ => AppError::Internal(e),
    }
}

//! Name-based sandbox lookup and get-or-create — split out from
//! `routes_sandbox.rs` since this crosses into snapshot territory
//! (resuming a name that currently resolves to a held snapshot) and needs
//! `AppState::lock_name`, a distinct enough concern from plain create/
//! list/stop lifecycle management to earn its own file (see
//! `routes_sandbox.rs`'s module doc comment and root `AGENTS.md`'s
//! file-splitting precedent).
//!
//! Two routes:
//! - `GET /sandboxes/by-name/:name` resolves a name to a *live* sandbox's
//!   id, so a caller who created a sandbox with a name doesn't have to
//!   track the opaque id itself to act on it later. Deliberately narrow:
//!   it does not resume a stopped (snapshotted) sandbox on the caller's
//!   behalf — that's a real side effect (booting a VM) a plain `GET`
//!   shouldn't perform implicitly, so a name that currently resolves to a
//!   held snapshot instead is a `409` pointing at `get-or-create` (below)
//!   rather than a silent resume or a bare `404` that hides *why* it
//!   can't be found.
//! - `POST /sandboxes/get-or-create` is the "resume-or-create by name in
//!   one call" primitive the ROADMAP asks for. Shaped as its own endpoint
//!   rather than a flag on `POST /sandboxes` or a by-name variant of
//!   `POST /snapshots/:id/resume`: get-or-create is a genuinely different
//!   operation from either — it can resolve to three different outcomes
//!   (return a live sandbox unchanged, resume a held snapshot, or create
//!   fresh) depending on state the caller doesn't need to inspect
//!   up front, and burying that behind a boolean flag on `POST
//!   /sandboxes` would make an idempotent, side-effect-aware operation
//!   look like an ordinary (non-idempotent) create with an obscure
//!   modifier. A dedicated endpoint documents the contract by existing.

use crate::error::AppError;
use crate::routes_sandbox::{create_sandbox_core, CreateSandboxRequest};
use crate::routes_snapshot::resume_snapshot_by_id;
use crate::state::{AppState, NameResolution};
use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Sandbox/snapshot names double as a URL path segment
/// (`GET /sandboxes/by-name/:name`) and, on disk, as part of a
/// snapshot's persisted metadata — kept restricted to characters safe in
/// both, mirroring `sandkiln_vmm::drive::validate_id`'s reasoning for
/// drive ids: no path separators, nothing that needs escaping.
pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid name '{name}': must be 1-64 characters, alphanumeric, '-', or '_' only"
        ))
    }
}

#[derive(Serialize)]
pub struct SandboxByNameResponse {
    id: String,
}

/// Resolves a name to a live sandbox's id. See the module doc comment for
/// why this deliberately does not resume a stopped (snapshotted) sandbox
/// on the caller's behalf.
pub async fn get_sandbox_by_name(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<SandboxByNameResponse>, AppError> {
    match state.resolve_name(&name) {
        Some(NameResolution::Live(id)) => Ok(Json(SandboxByNameResponse { id })),
        Some(NameResolution::Snapshot(_)) => Err(AppError::Conflict(format!(
            "sandbox '{name}' is currently stopped (snapshotted), not live — use POST /sandboxes/get-or-create \
             to resume it"
        ))),
        None => Err(AppError::NotFound(name)),
    }
}

#[derive(Deserialize)]
pub struct GetOrCreateSandboxRequest {
    name: String,
    #[serde(default)]
    tags: HashMap<String, String>,
    #[serde(default)]
    drives: Vec<crate::routes_drives::DriveAttachment>,
    #[serde(default)]
    vcpu_count: Option<u8>,
    #[serde(default)]
    mem_size_mib: Option<u32>,
}

#[derive(Serialize)]
pub struct GetOrCreateSandboxResponse {
    id: String,
    /// `true` only when this call actually booted a brand-new sandbox
    /// from the base rootfs — `false` for both "already live, returned
    /// unchanged" and "resumed from a held snapshot", since in both of
    /// those cases the caller is getting back *existing* state, not a
    /// fresh environment.
    created: bool,
}

/// Resolves a name to a sandbox in one call, creating it if it doesn't
/// exist yet: a live sandbox with this name is returned as-is; a held
/// snapshot with this name is resumed (consuming it, same as
/// `POST /snapshots/:id/resume`); otherwise a fresh sandbox is created
/// and given this name. `tags`/`drives`/`vcpu_count`/`mem_size_mib` are
/// used only for the create-fresh case — resuming an existing snapshot
/// always uses what was recorded on it when it was taken, exactly like
/// `POST /snapshots/:id/resume` accepts no overrides today.
///
/// Race-safe under concurrent calls with the same brand-new name: the
/// whole check-then-act sequence runs under `AppState::lock_name(name)`,
/// so a second concurrent call for the same name blocks until the first
/// either commits its claim (the second then finds it live and returns
/// the same id, `created: false`) or fails outright (the name is free
/// again). See `AppState::lock_name`'s doc comment.
#[tracing::instrument(skip(state, request), fields(name = %request.name))]
pub async fn get_or_create_sandbox(
    State(state): State<Arc<AppState>>,
    Json(request): Json<GetOrCreateSandboxRequest>,
) -> Result<Json<GetOrCreateSandboxResponse>, AppError> {
    validate_name(&request.name).map_err(AppError::BadRequest)?;
    let _guard = state.lock_name(&request.name).await;

    match state.resolve_name(&request.name) {
        Some(NameResolution::Live(id)) => Ok(Json(GetOrCreateSandboxResponse { id, created: false })),
        Some(NameResolution::Snapshot(snapshot_id)) => {
            let id = resume_snapshot_by_id(state, snapshot_id).await?;
            Ok(Json(GetOrCreateSandboxResponse { id, created: false }))
        }
        None => {
            let create_request = CreateSandboxRequest {
                name: Some(request.name),
                tags: request.tags,
                drives: request.drives,
                vcpu_count: request.vcpu_count,
                mem_size_mib: request.mem_size_mib,
            };
            let id = create_sandbox_core(&state, create_request).await?;
            Ok(Json(GetOrCreateSandboxResponse { id, created: true }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_name_accepts_a_reasonable_name() {
        assert!(validate_name("web-server_1").is_ok());
        assert!(validate_name("a").is_ok());
        assert!(validate_name(&"x".repeat(64)).is_ok());
    }

    #[test]
    fn validate_name_rejects_empty() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn validate_name_rejects_too_long() {
        assert!(validate_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn validate_name_rejects_a_path_separator() {
        let err = validate_name("foo/bar").unwrap_err();
        assert!(err.contains("foo/bar"), "message was: {err}");
    }

    #[test]
    fn validate_name_rejects_whitespace_and_other_punctuation() {
        assert!(validate_name("has space").is_err());
        assert!(validate_name("has.dot").is_err());
        assert!(validate_name("has:colon").is_err());
    }
}

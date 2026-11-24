//! Sandbox lifecycle: create, list, stop. Exec/file ops live in
//! `routes::exec` (shares `call_agent`, unrelated to lifecycle). Name
//! lookup/get-or-create lives in `routes::sandbox_name` (crosses into
//! snapshot territory, needs the per-name lock).

use crate::http::error::AppError;
use crate::http::routes::drives::DriveAttachment;
use crate::lifecycle::create::create_sandbox_core;
use crate::lifecycle::stop::{stop_sandbox_by_id, StopError, StopOutcome};
use crate::snapshots::ops::take::SnapshotBlocked;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

#[derive(Deserialize, Default)]
pub struct CreateSandboxRequest {
    /// Unique among live sandboxes + held snapshots when claimed (`409`
    /// otherwise). Optional. See `Sandbox::name`, `routes::sandbox_name`.
    #[serde(default)]
    pub(crate) name: Option<String>,
    #[serde(default)]
    pub(crate) tags: HashMap<String, String>,
    /// Existing persistent drives (`POST /drives`) to attach at boot.
    #[serde(default)]
    pub(crate) drives: Vec<DriveAttachment>,
    /// Overrides `SANDKILN_VCPU_COUNT` for this sandbox. `400` (not
    /// clamped) if `0` or above `SANDKILN_MAX_VCPU_COUNT` — see
    /// `resolve_resource_override`.
    #[serde(default)]
    pub(crate) vcpu_count: Option<u8>,
    /// Overrides `SANDKILN_MEM_SIZE_MIB`; same rules as `vcpu_count`.
    #[serde(default)]
    pub(crate) mem_size_mib: Option<u32>,
    /// Boots from a registered image (`POST /images`) instead of
    /// `SANDKILN_BASE_ROOTFS`. `404` if the id isn't registered.
    #[serde(default)]
    pub(crate) image_id: Option<String>,
    /// Firecracker's own token-bucket I/O rate limiter, applied to the
    /// rootfs drive, every attached drive, and the network interface.
    /// Omitted = unlimited. At least one sub-field must be set and
    /// non-zero, or `400`.
    #[serde(default)]
    pub(crate) rate_limit: Option<RateLimitRequest>,
    /// Outbound network policy — see `sandkiln_vmm::egress`. Omitted =
    /// unrestricted (today's default). Unlike `drives`/`rate_limit`, can
    /// still match a pre-warmed pool: enforced via iptables applied
    /// *after* boot/resume, never baked into Firecracker's VM state.
    #[serde(default)]
    pub(crate) egress: Option<EgressPolicyRequest>,
    /// Baked in for the sandbox's lifetime; base layer under every
    /// `exec`/`exec-stream` call's own `env` (call wins on conflict —
    /// see `routes::exec::resolve_env`). Persists through
    /// snapshot/resume/fork like `tags`.
    #[serde(default)]
    pub(crate) env: HashMap<String, String>,
}

#[derive(Deserialize, Clone, Copy)]
pub struct RateLimitRequest {
    #[serde(default)]
    pub(crate) bandwidth_bytes_per_sec: Option<u64>,
    #[serde(default)]
    pub(crate) ops_per_sec: Option<u64>,
}

#[derive(Deserialize, Clone)]
pub struct EgressPolicyRequest {
    pub(crate) mode: EgressModeRequest,
    /// IPv4 CIDRs. Validated in `resolve_egress_policy`, not here, so a
    /// bad one gets one clear `400` naming it.
    #[serde(default)]
    pub(crate) allow_cidrs: Vec<String>,
    #[serde(default)]
    pub(crate) deny_cidrs: Vec<String>,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum EgressModeRequest {
    AllowAll,
    DenyAll,
}

#[derive(Serialize)]
pub struct CreateSandboxResponse {
    id: String,
}
#[tracing::instrument(skip(state, body))]
pub async fn create_sandbox(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<CreateSandboxResponse>, AppError> {
    let request: CreateSandboxRequest = if body.is_empty() {
        CreateSandboxRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| AppError::BadRequest(format!("invalid request body: {e}")))?
    };

    // Serializes this call against any other concurrent claim of the
    // same name so two racing callers can't both pass the uniqueness
    // check below. See `AppState::lock_name`.
    let _name_guard = match &request.name {
        Some(name) => {
            crate::http::routes::sandbox_name::validate_name(name).map_err(AppError::BadRequest)?;
            let guard = state.lock_name(name).await;
            if let Some(holder) = state.name_holder(name) {
                return Err(AppError::Conflict(format!("name '{name}' is already used by {holder}")));
            }
            Some(guard)
        }
        None => None,
    };

    let id = create_sandbox_core(&state, request).await?;
    Ok(Json(CreateSandboxResponse { id }))
}

#[derive(Serialize)]
pub struct SandboxSummary {
    id: String,
    created_at_unix: u64,
    tags: HashMap<String, String>,
    name: Option<String>,
}

#[derive(Serialize)]
pub struct ListSandboxesResponse {
    sandboxes: Vec<SandboxSummary>,
}

/// `?tag.<key>=<value>` query params filter; a sandbox must match all given.
pub async fn list_sandboxes(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<ListSandboxesResponse> {
    let tag_filters: Vec<(&str, &str)> = query
        .iter()
        .filter_map(|(k, v)| k.strip_prefix("tag.").map(|key| (key, v.as_str())))
        .collect();

    let sandboxes = state
        .sandboxes
        .lock()
        .unwrap()
        .values()
        .filter(|s| tag_filters.iter().all(|(k, v)| s.tags.get(*k).map(String::as_str) == Some(v)))
        .map(|s| SandboxSummary {
            id: s.id.clone(),
            created_at_unix: s.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            tags: s.tags.clone(),
            name: s.name.clone(),
        })
        .collect();
    Json(ListSandboxesResponse { sandboxes })
}

#[derive(Serialize)]
pub struct HistoryRecordBody {
    id: String,
    name: Option<String>,
    tags: HashMap<String, String>,
    image_id: Option<String>,
    created_at_unix: u64,
    ended_at_unix: Option<u64>,
    end_reason: Option<String>,
    final_snapshot_id: Option<String>,
}

impl From<sandkiln_store::HistoryRecord> for HistoryRecordBody {
    fn from(r: sandkiln_store::HistoryRecord) -> Self {
        Self {
            id: r.id,
            name: r.name,
            tags: r.tags,
            image_id: r.image_id,
            created_at_unix: r.created_at_unix,
            ended_at_unix: r.ended_at_unix,
            end_reason: r.end_reason,
            final_snapshot_id: r.final_snapshot_id,
        }
    }
}

#[derive(Serialize)]
pub struct SandboxHistoryResponse {
    history: Vec<HistoryRecordBody>,
}

/// Durable history, independent of the live `sandboxes` map — survives a
/// restart, unlike `GET /sandboxes` (see `sandkiln-store`'s doc comment:
/// it cannot revive a stopped sandbox). `?live_only=`/`?limit=` (default
/// `sandkiln_store::DEFAULT_LIST_LIMIT`). Newest-created first.
pub async fn sandbox_history(
    State(state): State<Arc<AppState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<SandboxHistoryResponse>, AppError> {
    let live_only = query.get("live_only").map(|v| v == "true");
    let limit = query.get("limit").and_then(|v| v.parse::<u32>().ok());
    let filter = sandkiln_store::HistoryFilter { live_only, limit };

    let records = spawn_blocking_in_current_span("history list task panicked", {
        let state = state.clone();
        move || state.history.list(&filter)
    })
    .await
    .map_err(|e| AppError::Internal(std::io::Error::other(e.to_string())))?;

    Ok(Json(SandboxHistoryResponse { history: records.into_iter().map(HistoryRecordBody::from).collect() }))
}

/// `DELETE /sandboxes/:id?keep=false` query-string parse. Pure, for unit
/// testing (mirrors `resolve_resource_override`).
fn parse_keep(params: &HashMap<String, String>) -> Result<bool, String> {
    match params.get("keep").map(String::as_str) {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => Err(format!("invalid 'keep' query parameter '{other}': expected 'true' or 'false'")),
    }
}

#[derive(Serialize)]
pub struct StopSandboxResponse {
    /// Whether this stop produced a new resumable `Snapshot`. `false`
    /// either from `?keep=false` or because this sandbox had nothing new
    /// to preserve (a fork — see `stop_sandbox_by_id`).
    kept: bool,
    snapshot_id: Option<String>,
}

/// `DELETE /sandboxes/:id`. Default response is `200` with a JSON body
/// (there's now a snapshot id worth returning, per "persistent by
/// default" — see `stop_sandbox_by_id`); `?keep=false` keeps the
/// original bare `204` contract.
#[tracing::instrument(skip(state))]
pub async fn stop_sandbox(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<axum::response::Response, AppError> {
    let keep = parse_keep(&params).map_err(AppError::BadRequest)?;

    let outcome = stop_sandbox_by_id(state, id.clone(), keep).await.map_err(|e| match e {
        StopError::NotFound => AppError::NotFound(id),
        StopError::CannotPreserve(reason) => cannot_preserve_error(reason),
        StopError::Io(e) => AppError::from(e),
    })?;

    if !keep {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let (kept, snapshot_id) = match outcome {
        StopOutcome::Snapshotted(snapshot_id) => (true, Some(snapshot_id)),
        StopOutcome::Destroyed => (false, None),
    };
    Ok(Json(StopSandboxResponse { kept, snapshot_id }).into_response())
}

fn cannot_preserve_error(reason: SnapshotBlocked) -> AppError {
    match reason {
        SnapshotBlocked::Jailed => AppError::Conflict(
            "this sandbox is jailed, and snapshotting a jailed sandbox is not supported yet — it can't be \
             stopped-and-preserved by default; retry with ?keep=false to destroy it instead"
                .to_string(),
        ),
        // Unreachable via `stop_sandbox_by_id` today (forks are a silent
        // destroy, not this error) — kept exhaustive so a future change
        // fails to compile instead of panicking if it ever does reach here.
        SnapshotBlocked::ForkedFrom(source) => AppError::Conflict(format!(
            "this sandbox was forked from snapshot {source} and can't be independently snapshotted — retry with \
             ?keep=false to destroy it instead"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keep_defaults_to_true_when_absent() {
        assert_eq!(parse_keep(&HashMap::new()), Ok(true));
    }

    #[test]
    fn parse_keep_accepts_explicit_true_and_false() {
        assert_eq!(parse_keep(&HashMap::from([("keep".to_string(), "true".to_string())])), Ok(true));
        assert_eq!(parse_keep(&HashMap::from([("keep".to_string(), "false".to_string())])), Ok(false));
    }

    #[test]
    fn parse_keep_rejects_anything_else() {
        let err = parse_keep(&HashMap::from([("keep".to_string(), "yes".to_string())])).unwrap_err();
        assert!(err.contains("yes"), "message was: {err}");
    }

    #[test]
    fn cannot_preserve_error_for_jailed_mentions_the_opt_out() {
        let AppError::Conflict(message) = cannot_preserve_error(SnapshotBlocked::Jailed) else {
            panic!("expected Conflict")
        };
        assert!(message.contains("keep=false"), "message was: {message}");
    }
}

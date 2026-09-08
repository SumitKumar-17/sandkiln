//! Filesystem metadata/structure operations against a running sandbox:
//! chmod, chown, mkdir, rename, copy, symlink, readlink, truncate, and
//! directory listing. Split out of `routes_exec` (which keeps exec and
//! whole-file read/write) since these are a real, independently-workable
//! seam — data transfer vs. filesystem structure — and together they'd
//! have pushed that file well past this crate's usual size range. Every
//! handler here is the same shape: forward one request to the guest
//! agent via `routes_exec::call_agent`, map its response.
//!
//! None of these validate `path` before forwarding it — same as
//! `routes_exec::read_file`/`write_file` already don't. The guest agent
//! is a deliberately "dumb executor" (see its own `AGENTS.md`); a path
//! is scoped to whatever it resolves to *inside that one microVM's own
//! filesystem*, not the real host, so there's nothing to jail against
//! from here that KVM/Firecracker's own isolation doesn't already cover.

use crate::error::AppError;
use crate::routes_exec::call_agent;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use sandkiln_protocol::{DirEntry, Request, Response as AgentResponse};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Decodes a guest-agent response that carries no data of its own —
/// every handler below except `list_dir`/`readlink` reduces to this.
fn ok_or_bad_request(response: AgentResponse) -> Result<StatusCode, AppError> {
    match response {
        AgentResponse::Ok => Ok(StatusCode::NO_CONTENT),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

#[derive(Deserialize)]
pub struct ChmodRequestBody {
    path: String,
    mode: u32,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path, mode = %body.mode))]
pub async fn chmod(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<ChmodRequestBody>) -> Result<StatusCode, AppError> {
    ok_or_bad_request(call_agent(state, id, Request::Chmod { path: body.path, mode: body.mode }).await?)
}

#[derive(Deserialize)]
pub struct ChownRequestBody {
    path: String,
    uid: u32,
    gid: u32,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path, uid = %body.uid, gid = %body.gid))]
pub async fn chown(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<ChownRequestBody>) -> Result<StatusCode, AppError> {
    ok_or_bad_request(call_agent(state, id, Request::Chown { path: body.path, uid: body.uid, gid: body.gid }).await?)
}

#[derive(Deserialize)]
pub struct MkdirRequestBody {
    path: String,
    #[serde(default)]
    parents: bool,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path, parents = %body.parents))]
pub async fn mkdir(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<MkdirRequestBody>) -> Result<StatusCode, AppError> {
    ok_or_bad_request(call_agent(state, id, Request::Mkdir { path: body.path, parents: body.parents }).await?)
}

#[derive(Deserialize)]
pub struct RenameRequestBody {
    from: String,
    to: String,
}

#[tracing::instrument(skip(state, body), fields(from = %body.from, to = %body.to))]
pub async fn rename(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<RenameRequestBody>) -> Result<StatusCode, AppError> {
    ok_or_bad_request(call_agent(state, id, Request::Rename { from: body.from, to: body.to }).await?)
}

#[derive(Deserialize)]
pub struct CopyRequestBody {
    from: String,
    to: String,
}

#[tracing::instrument(skip(state, body), fields(from = %body.from, to = %body.to))]
pub async fn copy(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<CopyRequestBody>) -> Result<StatusCode, AppError> {
    ok_or_bad_request(call_agent(state, id, Request::Copy { from: body.from, to: body.to }).await?)
}

#[derive(Deserialize)]
pub struct SymlinkRequestBody {
    target: String,
    link_path: String,
}

#[tracing::instrument(skip(state, body), fields(target = %body.target, link_path = %body.link_path))]
pub async fn symlink(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<SymlinkRequestBody>) -> Result<StatusCode, AppError> {
    ok_or_bad_request(call_agent(state, id, Request::Symlink { target: body.target, link_path: body.link_path }).await?)
}

#[derive(Deserialize)]
pub struct ReadlinkRequestBody {
    path: String,
}

#[derive(Serialize)]
pub struct ReadlinkResponseBody {
    target: String,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path))]
pub async fn readlink(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ReadlinkRequestBody>,
) -> Result<Json<ReadlinkResponseBody>, AppError> {
    match call_agent(state, id, Request::Readlink { path: body.path }).await? {
        AgentResponse::Link { target } => Ok(Json(ReadlinkResponseBody { target })),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

#[derive(Deserialize)]
pub struct TruncateRequestBody {
    path: String,
    size: u64,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path, size = %body.size))]
pub async fn truncate(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<TruncateRequestBody>) -> Result<StatusCode, AppError> {
    ok_or_bad_request(call_agent(state, id, Request::Truncate { path: body.path, size: body.size }).await?)
}

#[derive(Deserialize)]
pub struct ListDirRequestBody {
    path: String,
}

/// Deliberately its own type rather than reusing `sandkiln_protocol::DirEntry`
/// directly as the HTTP response shape — the vsock wire protocol and the
/// public HTTP API are separate contracts; coupling them would mean a
/// future guest-agent-side protocol change forces an HTTP breaking change
/// for no reason. Field-for-field identical today, free to diverge later.
#[derive(Serialize)]
pub struct DirEntryBody {
    name: String,
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    mode: u32,
    mtime_unix: u64,
}

impl From<DirEntry> for DirEntryBody {
    fn from(e: DirEntry) -> Self {
        Self { name: e.name, is_dir: e.is_dir, is_symlink: e.is_symlink, size: e.size, mode: e.mode, mtime_unix: e.mtime_unix }
    }
}

#[derive(Serialize)]
pub struct ListDirResponseBody {
    entries: Vec<DirEntryBody>,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path))]
pub async fn list_dir(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ListDirRequestBody>,
) -> Result<Json<ListDirResponseBody>, AppError> {
    match call_agent(state, id, Request::ListDir { path: body.path }).await? {
        AgentResponse::Dir { entries } => Ok(Json(ListDirResponseBody { entries: entries.into_iter().map(DirEntryBody::from).collect() })),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

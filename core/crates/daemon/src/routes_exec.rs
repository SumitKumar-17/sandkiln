//! Exec and file operations against a running sandbox — everything that's
//! just forwarding one request to the guest agent over vsock and reporting
//! its response, via the shared `call_agent` helper.

use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use sandkiln_protocol::{Request, Response as AgentResponse};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

#[derive(Deserialize)]
pub struct ExecRequestBody {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Serialize)]
pub struct ExecResponseBody {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

#[tracing::instrument(skip(state, body), fields(command = %body.command))]
pub async fn exec(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ExecRequestBody>,
) -> Result<Json<ExecResponseBody>, AppError> {
    let response = call_agent(state, id, Request::Exec { command: body.command, args: body.args }).await?;

    match response {
        AgentResponse::Exec { stdout, stderr, exit_code } => Ok(Json(ExecResponseBody { stdout, stderr, exit_code })),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

#[derive(Deserialize)]
pub struct ReadFileRequestBody {
    path: String,
}

#[derive(Serialize)]
pub struct ReadFileResponseBody {
    content_base64: String,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path))]
pub async fn read_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ReadFileRequestBody>,
) -> Result<Json<ReadFileResponseBody>, AppError> {
    let response = call_agent(state, id, Request::ReadFile { path: body.path }).await?;
    match response {
        AgentResponse::File { content_base64 } => Ok(Json(ReadFileResponseBody { content_base64 })),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

#[derive(Deserialize)]
pub struct WriteFileRequestBody {
    path: String,
    content_base64: String,
}

#[tracing::instrument(skip(state, body), fields(path = %body.path))]
pub async fn write_file(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<WriteFileRequestBody>,
) -> Result<StatusCode, AppError> {
    let response =
        call_agent(state, id, Request::WriteFile { path: body.path, content_base64: body.content_base64 }).await?;
    match response {
        AgentResponse::Ok => Ok(StatusCode::NO_CONTENT),
        AgentResponse::Error { message } => Err(AppError::BadRequest(message)),
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

/// Shared by every route that just forwards one request to the guest
/// agent and reports its response — exec/read/write all fit this shape.
async fn call_agent(state: Arc<AppState>, id: String, request: Request) -> Result<AgentResponse, AppError> {
    tokio::task::spawn_blocking(move || {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        *sandbox.last_activity.lock().unwrap() = Instant::now();
        let started = Instant::now();
        let response = sandbox.vm.call(&request).map_err(AppError::from)?;
        state.metrics.record_exec_latency_ms(started.elapsed().as_secs_f64() * 1000.0);
        Ok(response)
    })
    .await
    .expect("agent call task panicked")
}

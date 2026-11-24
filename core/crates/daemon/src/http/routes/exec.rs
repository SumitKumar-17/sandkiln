//! Exec and whole-file read/write against a running sandbox. Filesystem
//! metadata/structure operations (chmod, chown, mkdir, rename, copy,
//! symlink, readlink, truncate, directory listing) live in `routes::fs`
//! instead — a real seam (data transfer vs. filesystem structure), split
//! out once this file would otherwise have grown well past the ~300-line
//! range adding all of them here. Both files share the `call_agent`
//! helper below — everything on both sides is just forwarding one
//! request to the guest agent over vsock and reporting its response.

use crate::http::error::AppError;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use sandkiln_protocol::{Request, Response as AgentResponse};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
pub struct ExecRequestBody {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    /// Merged on top of the sandbox's own create-time `env` (this wins on
    /// a key conflict) — see `resolve_env`. Omitted entirely means "just
    /// the sandbox's own defaults, unchanged," not "clear them."
    #[serde(default)]
    env: HashMap<String, String>,
    /// Wall-clock limit enforced inside the guest: on expiry the command's
    /// whole process group is killed and the reply is a 504 with
    /// `timed_out: true` and the output captured so far. Omitted means the
    /// daemon's `exec_timeout_secs`, which is also the most that may be asked.
    #[serde(default)]
    timeout_secs: Option<u64>,
}

/// How much longer than the command's own limit the daemon waits for the
/// guest agent's reply: the agent needs up to its SIGTERM grace plus kill
/// settle time (a few seconds) to report a timeout, and it should answer
/// first so the caller gets the partial output, not a bare gateway error.
const AGENT_REPLY_MARGIN: Duration = Duration::from_secs(10);

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
) -> Result<Response, AppError> {
    let limit = resolve_exec_timeout(body.timeout_secs, state.config.exec_timeout)?;
    let env = resolve_env(&state, &id, body.env)?;
    let request = Request::Exec { command: body.command, args: body.args, env, timeout_secs: Some(limit.as_secs()) };
    let response = call_agent_with_timeout(state, id, request, limit + AGENT_REPLY_MARGIN).await?;

    match response {
        AgentResponse::Exec { stdout, stderr, exit_code } => Ok(Json(ExecResponseBody { stdout, stderr, exit_code }).into_response()),
        AgentResponse::TimedOut { stdout, stderr } => {
            let (status, body) = timed_out_body(limit, stdout, stderr);
            Ok((status, Json(body)).into_response())
        }
        other => Err(AppError::Internal(std::io::Error::other(format!("unexpected agent response: {other:?}")))),
    }
}

/// The limit a request runs under: its own `timeout_secs`, or the daemon's
/// ceiling when absent. Zero and anything above the ceiling are the caller's
/// mistake (400), not silently clamped, so a client never believes it
/// granted a command more time than it got.
fn resolve_exec_timeout(requested: Option<u64>, ceiling: Duration) -> Result<Duration, AppError> {
    let Some(secs) = requested else { return Ok(ceiling) };
    if secs == 0 {
        return Err(AppError::BadRequest("timeout_secs must be at least 1".to_string()));
    }
    if secs > ceiling.as_secs() {
        return Err(AppError::BadRequest(format!(
            "timeout_secs {secs} exceeds this daemon's limit of {}s (exec_timeout_secs)",
            ceiling.as_secs()
        )));
    }
    Ok(Duration::from_secs(secs))
}

/// The 504 body for a command the guest killed at its limit. Carries the
/// usual `error` string (every error body has one) plus the partial output
/// and `timed_out: true`, which is how a client tells this apart from the
/// 504 of a host-side wait that expired without the guest answering.
fn timed_out_body(limit: Duration, stdout: String, stderr: String) -> (StatusCode, serde_json::Value) {
    (
        StatusCode::GATEWAY_TIMEOUT,
        serde_json::json!({
            "error": format!("command timed out after {}s and was killed", limit.as_secs()),
            "timed_out": true,
            "timeout_secs": limit.as_secs(),
            "stdout": stdout,
            "stderr": stderr,
        }),
    )
}

/// Merges a sandbox's create-time `env` (the base layer) with a per-call
/// override (`call_env`, wins on a key conflict) into the one map the
/// guest agent actually receives — the guest agent has no notion of
/// "sandbox-level" vs. "call-level" env, it just executes with whatever
/// `Request::Exec::env`/`ExecStreamHandshake::env` says (see this crate's
/// `AGENTS.md`). A quick, synchronous lock (a `HashMap` clone, not I/O),
/// separate from `call_agent`'s own lock acquisition inside its blocking
/// task -- cheap enough not to need `spawn_blocking` of its own, same
/// reasoning already applied to other short `state.sandboxes.lock()`
/// reads elsewhere in this daemon.
pub(crate) fn resolve_env(state: &AppState, id: &str, call_env: HashMap<String, String>) -> Result<HashMap<String, String>, AppError> {
    let sandboxes = state.sandboxes.lock().unwrap();
    let sandbox = sandboxes.get(id).ok_or_else(|| AppError::NotFound(id.to_string()))?;
    let mut env = sandbox.env.clone();
    env.extend(call_env);
    Ok(env)
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
/// agent and reports its response — exec/read/write here and every
/// `routes::fs` handler all fit this shape.
pub(crate) async fn call_agent(state: Arc<AppState>, id: String, request: Request) -> Result<AgentResponse, AppError> {
    let timeout = state.config.exec_timeout;
    call_agent_with_timeout(state, id, request, timeout).await
}

/// [`call_agent`] with a caller-chosen bound on the guest's reply — `exec`
/// derives it from the command's own limit.
pub(crate) async fn call_agent_with_timeout(
    state: Arc<AppState>,
    id: String,
    request: Request,
    timeout: Duration,
) -> Result<AgentResponse, AppError> {
    // Copy the endpoint out and release the sandbox map's lock *before* the
    // call: a user command can run for minutes, and holding the daemon-wide
    // lock across it would freeze every other sandbox operation meanwhile.
    let endpoint = {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        *sandbox.last_activity.lock().unwrap() = Instant::now();
        sandbox.vm.agent_endpoint()
    };
    spawn_blocking_in_current_span("agent call task panicked", move || {
        let started = Instant::now();
        let response = endpoint
            .call_with_timeout(&request, Some(timeout))
            .map_err(|e| agent_call_error(e, timeout))?;
        state.metrics.record_exec_latency_ms(started.elapsed().as_secs_f64() * 1000.0);
        Ok(response)
    })
    .await
}

/// A reply that never came within the bound is the caller's 504, not a 500:
/// the sandbox is fine, the command just has not finished.
fn agent_call_error(e: std::io::Error, timeout: std::time::Duration) -> AppError {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => AppError::GatewayTimeout(format!(
            "no reply from the sandbox within {}s; the command may still be running in the guest",
            timeout.as_secs()
        )),
        _ => AppError::from(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn omitted_timeout_defaults_to_the_ceiling() {
        assert_eq!(resolve_exec_timeout(None, Duration::from_secs(600)).ok().unwrap(), Duration::from_secs(600));
    }

    #[test]
    fn a_timeout_up_to_the_ceiling_is_accepted() {
        assert_eq!(resolve_exec_timeout(Some(2), Duration::from_secs(600)).ok().unwrap(), Duration::from_secs(2));
        assert_eq!(resolve_exec_timeout(Some(600), Duration::from_secs(600)).ok().unwrap(), Duration::from_secs(600));
    }

    #[test]
    fn zero_and_over_ceiling_timeouts_are_400s() {
        for bad in [0, 601, u64::MAX] {
            let e = resolve_exec_timeout(Some(bad), Duration::from_secs(600)).unwrap_err();
            assert!(matches!(e, AppError::BadRequest(_)), "{bad} should be a 400");
        }
    }

    #[test]
    fn timed_out_body_is_a_504_carrying_partial_output() {
        let (status, body) = timed_out_body(Duration::from_secs(2), "out".to_string(), "err".to_string());
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(body["timed_out"], true);
        assert_eq!(body["timeout_secs"], 2);
        assert_eq!(body["stdout"], "out");
        assert_eq!(body["stderr"], "err");
        assert!(body["error"].as_str().unwrap().contains("2s"));
    }

    #[test]
    fn request_body_timeout_is_optional() {
        let b: ExecRequestBody = serde_json::from_str(r#"{"command":"ls"}"#).unwrap();
        assert_eq!(b.timeout_secs, None);
        let b: ExecRequestBody = serde_json::from_str(r#"{"command":"ls","timeout_secs":5}"#).unwrap();
        assert_eq!(b.timeout_secs, Some(5));
    }

    #[test]
    fn read_timeouts_become_504_not_500() {
        for kind in [ErrorKind::WouldBlock, ErrorKind::TimedOut] {
            let e = agent_call_error(Error::new(kind, "x"), Duration::from_secs(600));
            assert!(matches!(&e, AppError::GatewayTimeout(m) if m.contains("600s")), "expected a GatewayTimeout");
        }
    }

    #[test]
    fn other_io_errors_stay_internal() {
        let e = agent_call_error(Error::new(ErrorKind::ConnectionRefused, "x"), Duration::from_secs(5));
        assert!(matches!(e, AppError::Internal(_)));
    }
}

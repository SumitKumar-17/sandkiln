//! Streamed background exec sessions: `POST .../exec-stream` starts a
//! command detached in the guest, `GET .../exec-stream` lists sessions,
//! `GET .../exec-stream/:session_id/logs` (WebSocket) attaches to one —
//! `kiln logs -f`'s mechanism.
//!
//! Not a streaming variant of `routes_exec::exec` (which blocks for one
//! request's lifetime, one final blob) — this returns immediately with a
//! session id; the command keeps running and its output keeps being
//! captured into a [`crate::log_session::LogSession`] regardless of
//! whether anyone's attached, and any number of `logs` clients can
//! attach/detach over the session's life, each getting a full **replay,
//! then live tail**.
//!
//! The daemon does the buffering, not the guest: `start_exec_stream`
//! opens one long-lived vsock connection (`Vm::open_exec_stream`) and
//! spawns `pump_exec_stream` to read it for as long as it stays open —
//! the guest agent itself never tracks a session past its own one
//! connection.
//!
//! Doesn't survive a daemon restart or the sandbox stopping (in-memory,
//! tied to one `Sandbox` value, same scope as
//! `Sandbox::pty_session_count`). No holder-tracking or concurrency cap
//! like `routes_pty` needs — each session owns its own independent vsock
//! connection and child process, nothing to contend over.

use crate::error::AppError;
use crate::log_session::{LogEvent, LogSession};
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct StartExecStreamRequest {
    command: String,
    #[serde(default)]
    args: Vec<String>,
    /// Merged on top of the sandbox's own create-time `env` (this wins on
    /// a key conflict) -- same shape and reasoning as
    /// `routes_exec::ExecRequestBody::env`.
    #[serde(default)]
    env: std::collections::HashMap<String, String>,
}

#[derive(Serialize)]
pub struct StartExecStreamResponse {
    id: String,
}

/// Starts a new background exec session and returns its id immediately
/// — the command itself is very likely still running (or hasn't even
/// been spawned by the guest yet) by the time this responds. Attach to
/// `GET .../exec-stream/:id/logs` to actually see its output.
#[tracing::instrument(skip(state, request), fields(sandbox_id = %id, command = %request.command))]
pub async fn start_exec_stream(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<StartExecStreamRequest>,
) -> Result<Json<StartExecStreamResponse>, AppError> {
    if request.command.is_empty() {
        return Err(AppError::BadRequest("command must not be empty".to_string()));
    }

    let session_id = Uuid::new_v4().to_string();
    let session = Arc::new(LogSession::new(session_id.clone(), request.command.clone(), request.args.clone()));

    // Opening the vsock connection and registering the session both
    // happen inside this one blocking task, under the sandboxes lock —
    // so a concurrent `DELETE /sandboxes/:id` either sees this session
    // registered or doesn't, never a half-registered one, and a caller
    // never gets back a session id for a sandbox that turned out not to
    // exist.
    let stream = spawn_blocking_in_current_span("open exec-stream task panicked", {
        let state = state.clone();
        let id = id.clone();
        let session = session.clone();
        move || -> Result<std::os::unix::net::UnixStream, AppError> {
            let sandboxes = state.sandboxes.lock().unwrap();
            let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
            let mut env = sandbox.env.clone();
            env.extend(request.env.clone());
            let stream = sandbox.vm.open_exec_stream(&request.command, &request.args, &env).map_err(AppError::from)?;
            sandbox.log_sessions.lock().unwrap().insert(session.id.clone(), session.clone());
            Ok(stream)
        }
    })
    .await?;

    // Detached on purpose: this pump must keep running for as long as
    // the guest connection stays open, well past this HTTP request's own
    // lifetime — see this file's module doc comment.
    tokio::task::spawn_blocking(move || pump_exec_stream(stream, session));

    Ok(Json(StartExecStreamResponse { id: session_id }))
}

#[derive(Serialize)]
pub struct ExecStreamSummary {
    id: String,
    command: String,
    args: Vec<String>,
    started_at_unix: u64,
    /// `null` while still running — the same "absence means in
    /// progress" convention `RetiredSnapshotSummary` etc. don't need
    /// (those only ever exist once something is already finished), but
    /// natural here since a session is listed from the moment it starts.
    exit_code: Option<i32>,
}

#[derive(Serialize)]
pub struct ListExecStreamsResponse {
    sessions: Vec<ExecStreamSummary>,
}

pub async fn list_exec_streams(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ListExecStreamsResponse>, AppError> {
    let sandboxes = state.sandboxes.lock().unwrap();
    let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
    let sessions = sandbox
        .log_sessions
        .lock()
        .unwrap()
        .values()
        .map(|s| ExecStreamSummary {
            id: s.id.clone(),
            command: s.command.clone(),
            args: s.args.clone(),
            started_at_unix: s.started_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            exit_code: s.exit_code(),
        })
        .collect();
    Ok(Json(ListExecStreamsResponse { sessions }))
}

/// Upgrades to a WebSocket and streams one session's output: a replay of
/// everything captured so far, then a live tail of anything new, until
/// either the process exits (the connection is then closed after a final
/// notice) or the client disconnects. Can be called again any number of
/// times for the same session — including after the process has already
/// finished, in which case it's just a replay with no live tail to wait
/// for.
#[tracing::instrument(skip(state), fields(sandbox_id = %id, session_id = %session_id))]
pub async fn attach_logs(
    State(state): State<Arc<AppState>>,
    Path((id, session_id)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let session = {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        let found = sandbox.log_sessions.lock().unwrap().get(&session_id).cloned();
        found.ok_or_else(|| AppError::NotFound(session_id.clone()))?
    };

    Ok(ws.on_upgrade(move |socket| async move { stream_logs(socket, session).await }))
}

/// Reads framed `ExecStreamEvent`s off `stream` (opened by
/// `start_exec_stream`, not tied to any client) until `Exit` or the
/// connection drops, appending each chunk into `session` — what makes
/// output available for replay/live-tail regardless of whether anyone's
/// attached. Blocking task (`std::os::unix::net::UnixStream`, not
/// tokio's), same reasoning as `routes_exec::call_agent`.
fn pump_exec_stream(mut stream: std::os::unix::net::UnixStream, session: Arc<LogSession>) {
    use base64::Engine;
    loop {
        let raw = match sandkiln_protocol::read_message(&mut stream) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(session_id = %session.id, error = %e, "exec-stream connection ended without a clean exit");
                session.finish(-1);
                return;
            }
        };
        match sandkiln_protocol::decode_exec_stream_event(&raw) {
            Ok(sandkiln_protocol::ExecStreamEvent::Stdout { data_base64 } | sandkiln_protocol::ExecStreamEvent::Stderr { data_base64 }) => {
                match base64::engine::general_purpose::STANDARD.decode(&data_base64) {
                    Ok(bytes) => session.append(&bytes),
                    Err(e) => tracing::warn!(session_id = %session.id, error = %e, "dropped an exec-stream chunk with invalid base64"),
                }
            }
            Ok(sandkiln_protocol::ExecStreamEvent::Exit { exit_code }) => {
                session.finish(exit_code);
                return;
            }
            Err(e) => {
                tracing::warn!(session_id = %session.id, error = %e, "malformed exec-stream event from the guest");
                session.finish(-1);
                return;
            }
        }
    }
}

/// The WebSocket side of one attach: replay, then live tail, then a
/// final notice once the process exits (whether that happens while this
/// client is still attached, or was already true when it connected).
async fn stream_logs(ws: WebSocket, session: Arc<LogSession>) {
    let (replay, truncated, exit_code, mut rx) = session.subscribe();
    let (mut sink, mut ws_stream) = ws.split();

    if truncated > 0 {
        let notice = format!("[... {truncated} earlier bytes truncated ...]\n");
        if sink.send(Message::Text(notice)).await.is_err() {
            return;
        }
    }
    if !replay.is_empty() && sink.send(Message::Binary(replay)).await.is_err() {
        return;
    }
    if let Some(code) = exit_code {
        let _ = sink.send(Message::Text(format!("[process exited with code {code}]\n"))).await;
        return;
    }

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(LogEvent::Data(bytes)) => {
                        if sink.send(Message::Binary(bytes)).await.is_err() {
                            break;
                        }
                    }
                    Ok(LogEvent::Exit(code)) => {
                        let _ = sink.send(Message::Text(format!("[process exited with code {code}]\n"))).await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        let _ = sink
                            .send(Message::Text(
                                "[... fell behind and some live output was dropped -- reconnect for the full log ...]\n".to_string(),
                            ))
                            .await;
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            // Detects the client disconnecting (or any other message
            // from them, which this endpoint has nothing to do with —
            // it's output-only, not interactive like `routes_pty`) so
            // this task doesn't outlive a client that's already gone.
            msg = ws_stream.next() => {
                if msg.is_none() {
                    break;
                }
            }
        }
    }
}

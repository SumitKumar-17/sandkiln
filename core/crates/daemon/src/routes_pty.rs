//! Interactive PTY sessions: `GET /sandboxes/:id/pty` upgrades to a
//! WebSocket and proxies raw bytes between it and a real shell running
//! inside the sandbox, over a dedicated vsock connection
//! (`sandkiln_vmm::vm::Vm::open_pty`) — a fundamentally different shape
//! from every other route in this crate, which are all one-request-one-
//! response. See `core/crates/guest-agent/src/pty.rs`'s module doc
//! comment for the guest side of this.
//!
//! No path validation, no input sanitization beyond what's needed to
//! parse `cols`/`rows` — once a session is open it's a real shell with
//! whatever privileges the guest agent itself runs as, same trust model
//! as `exec`.

use crate::error::AppError;
use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A per-sandbox ceiling, not a daemon-wide one — see `Sandbox::pty_session_count`.
/// ROADMAP.md's own suggested default for "if/when this is built."
const MAX_PTY_SESSIONS_PER_SANDBOX: u32 = 64;

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Decrements `Sandbox::pty_session_count` when a session ends, however
/// it ends (clean close, error, or the whole async task being dropped).
/// Constructed only after the matching increment already happened under
/// `AppState::sandboxes`'s lock in `pty_session` below — never on its own.
struct PtySessionGuard {
    counter: Arc<AtomicU32>,
}

impl Drop for PtySessionGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tracing::instrument(skip(state))]
pub async fn pty_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let cols = query.get("cols").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_COLS);
    let rows = query.get("rows").and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_ROWS);

    // Checked, opened, and handshaked before the WebSocket upgrade
    // happens at all -- a caller gets a real HTTP error (404, 409) for a
    // missing sandbox or an already-full session cap, rather than a
    // WebSocket connection that immediately closes with no clear reason.
    let (std_stream, guard) = spawn_blocking_in_current_span("open pty task panicked", {
        let state = state.clone();
        let id = id.clone();
        move || -> Result<(std::os::unix::net::UnixStream, PtySessionGuard), AppError> {
            let sandboxes = state.sandboxes.lock().unwrap();
            let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;

            let current = sandbox.pty_session_count.load(Ordering::SeqCst);
            if current >= MAX_PTY_SESSIONS_PER_SANDBOX {
                return Err(AppError::Conflict(format!(
                    "sandbox {id} already has {MAX_PTY_SESSIONS_PER_SANDBOX} open PTY sessions, the configured limit — close one before opening another"
                )));
            }
            sandbox.pty_session_count.fetch_add(1, Ordering::SeqCst);
            let counter = sandbox.pty_session_count.clone();

            match sandbox.vm.open_pty(cols, rows) {
                Ok(stream) => Ok((stream, PtySessionGuard { counter })),
                Err(e) => {
                    // The increment above must not outlive a failed open
                    // -- there's no successful session for a guard drop
                    // to eventually balance it against.
                    counter.fetch_sub(1, Ordering::SeqCst);
                    Err(AppError::from(e))
                }
            }
        }
    })
    .await?;

    std_stream.set_nonblocking(true).map_err(AppError::from)?;
    let pty_stream = tokio::net::UnixStream::from_std(std_stream).map_err(AppError::from)?;

    Ok(ws.on_upgrade(move |socket| async move {
        // Held for exactly the session's lifetime -- dropped (and so
        // decremented) whenever this async block ends, on any exit path.
        let _guard = guard;
        proxy_pty(socket, pty_stream).await;
    }))
}

/// Pumps bytes in both directions between `ws` and `pty` until either
/// side closes or errors — at which point the whole session ends, same
/// as a real terminal disconnecting closes both directions at once.
async fn proxy_pty(ws: WebSocket, pty: tokio::net::UnixStream) {
    let (mut pty_read, mut pty_write) = pty.into_split();
    let (mut ws_sink, mut ws_stream) = ws.split();

    let pty_to_ws = async {
        let mut buf = [0u8; 8192];
        loop {
            match pty_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if ws_sink.send(Message::Binary(buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                }
            }
        }
    };

    let ws_to_pty = async {
        while let Some(Ok(msg)) = ws_stream.next().await {
            let data = match msg {
                Message::Binary(b) => b,
                Message::Text(t) => t.into_bytes(),
                Message::Close(_) => break,
                // Ping/Pong are handled by axum's WebSocket internally
                // before a caller ever sees them here.
                _ => continue,
            };
            if pty_write.write_all(&data).await.is_err() {
                break;
            }
        }
    };

    tokio::select! {
        _ = pty_to_ws => {},
        _ = ws_to_pty => {},
    }
}

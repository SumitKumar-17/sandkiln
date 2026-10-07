//! Local tunnel: exposes a service on the *caller's own machine* to code
//! running inside a sandbox — the reverse of dev-server preview
//! (`routes_preview.rs`), which exposes a service inside the sandbox
//! outward. `POST /sandboxes/:id/tunnel` tells the guest agent to start
//! listening on a guest port; `GET .../tunnel/:tunnel_id/ws` is the
//! WebSocket the SDK/CLI caller holds open for the tunnel's whole life,
//! multiplexing every connection the guest accepts over that one socket
//! back to the caller, which then proxies each one to a real local TCP
//! socket on its own machine.
//!
//! The `POST`/`DELETE` calls sit behind the normal bearer-token
//! middleware like any other API call. The WebSocket needs the same
//! `?token=` fallback `/preview` and `/pty` use, even though the thing
//! opening it is always SDK/CLI code, never a browser tab: neither
//! browsers' nor Node's native `WebSocket` constructor can set a custom
//! `Authorization` header at all, a limitation in the WebSocket API
//! itself, not something specific to a browser context.
//!
//! **Wire format over the WebSocket** (binary frames only): one byte op
//! (`0` = open, `1` = data, `2` = close), one byte `conn_id` length, that
//! many bytes of `conn_id`, then the rest of the frame is payload (only
//! meaningful for `data`). `open`/`close` travel daemon-to-caller only
//! (a new guest connection arrived / its guest side closed); `data`
//! travels both directions; `close` from the caller means its local
//! socket closed and the guest side should be torn down too. Both SDKs
//! implement this exact framing — see `packages/sdk/src/tunnel.ts`/
//! `packages/python/src/sandkiln/tunnel.py` (or their sync/async
//! equivalents) if changing it here.

use crate::error::AppError;
use crate::state::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use sandkiln_protocol::Request;
use sandkiln_vmm::tunnel::TunnelConnection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

/// `POST`/`DELETE` go through `main.rs`'s normal bearer-gated
/// `sandbox_routes` — merge this into that router.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sandboxes/:id/tunnel", post(create_tunnel))
        .route("/sandboxes/:id/tunnel/:tunnel_id", axum::routing::delete(delete_tunnel))
}

/// The WebSocket route needs `auth::require_preview_token` instead, same
/// reason as `routes_pty.rs`/`routes_logs.rs`'s own WebSocket routes:
/// neither a browser's nor Node's native `WebSocket` constructor can set
/// a custom `Authorization` header, so this needs the `?token=` fallback
/// too, even though (unlike PTY/preview) the thing opening it is always
/// SDK/CLI code, never a browser tab — the limitation is in the
/// WebSocket API itself, not who's using it. Merge into `main.rs`'s
/// `websocket_routes`, not `sandbox_routes`.
pub fn ws_router() -> Router<Arc<AppState>> {
    Router::new().route("/sandboxes/:id/tunnel/:tunnel_id/ws", get(tunnel_ws))
}

#[derive(Deserialize)]
pub struct CreateTunnelRequest {
    guest_port: u16,
}

#[derive(Serialize)]
pub struct TunnelResponse {
    tunnel_id: String,
    guest_port: u16,
}

pub async fn create_tunnel(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<CreateTunnelRequest>,
) -> Result<Json<TunnelResponse>, AppError> {
    let tunnel_id = Uuid::new_v4().to_string();
    let receiver = sandkiln_vmm::tunnel::register(tunnel_id.clone());

    let send_result = {
        let state = state.clone();
        let id = id.clone();
        let tunnel_id = tunnel_id.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let sandboxes = state.sandboxes.lock().unwrap();
            let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
            match sandbox.vm.call(&Request::StartTunnel { tunnel_id, guest_port: request.guest_port }) {
                Ok(sandkiln_protocol::Response::Ok) => Ok(()),
                Ok(sandkiln_protocol::Response::Error { message }) => Err(AppError::BadRequest(message)),
                Ok(_) => Err(AppError::Internal(io::Error::other("unexpected agent response to start_tunnel"))),
                Err(e) => Err(AppError::from(e)),
            }
        })
        .await
        .map_err(|e| AppError::Internal(io::Error::other(format!("start_tunnel task panicked: {e}"))))?
    };

    if let Err(e) = send_result {
        sandkiln_vmm::tunnel::unregister(&tunnel_id);
        return Err(e);
    }

    state.tunnels.lock().unwrap().insert(tunnel_id.clone(), receiver);
    Ok(Json(TunnelResponse { tunnel_id, guest_port: request.guest_port }))
}

pub async fn delete_tunnel(State(state): State<Arc<AppState>>, Path((id, tunnel_id)): Path<(String, String)>) -> Result<(), AppError> {
    sandkiln_vmm::tunnel::unregister(&tunnel_id);
    state.tunnels.lock().unwrap().remove(&tunnel_id);

    let sandboxes = state.sandboxes.lock().unwrap();
    let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
    // Best-effort, like every other "tell the guest to stop something"
    // call on an already-ending resource -- the registration above is
    // already gone either way, which is what actually stops new
    // connections from going anywhere.
    let _ = sandbox.vm.call(&Request::StopTunnel { tunnel_id });
    Ok(())
}

pub async fn tunnel_ws(
    State(state): State<Arc<AppState>>,
    Path((_id, tunnel_id)): Path<(String, String)>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let receiver = state
        .tunnels
        .lock()
        .unwrap()
        .remove(&tunnel_id)
        .ok_or_else(|| AppError::Conflict(format!("tunnel {tunnel_id} has no pending connection to attach to (already attached, or never created)")))?;

    Ok(ws.on_upgrade(move |socket| proxy_tunnel(socket, receiver)))
}

const OP_OPEN: u8 = 0;
const OP_DATA: u8 = 1;
const OP_CLOSE: u8 = 2;

fn encode_frame(op: u8, conn_id: &str, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(2 + conn_id.len() + payload.len());
    frame.push(op);
    frame.push(conn_id.len() as u8);
    frame.extend_from_slice(conn_id.as_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn decode_frame(data: &[u8]) -> Option<(u8, &str, &[u8])> {
    let op = *data.first()?;
    let len = *data.get(1)? as usize;
    let conn_id = std::str::from_utf8(data.get(2..2 + len)?).ok()?;
    let payload = data.get(2 + len..)?;
    Some((op, conn_id, payload))
}

/// Runs for the tunnel's whole life: forwards every guest-accepted
/// connection out over the WebSocket, and routes inbound frames back to
/// the matching connection. Two independent directions sharing one
/// `writers` map (new guest connections only ever insert/remove; inbound
/// `data`/`close` frames only ever look up) -- a `tokio::sync::Mutex`
/// since a write to a connection's socket has to `.await` while the lock
/// is held, which a plain `std::sync::Mutex` guard can't survive across.
async fn proxy_tunnel(ws: WebSocket, receiver: std::sync::mpsc::Receiver<TunnelConnection>) {
    let (ws_sink, mut ws_stream) = ws.split();
    let ws_sink = Arc::new(AsyncMutex::new(ws_sink));
    let writers: Arc<AsyncMutex<HashMap<String, tokio::net::unix::OwnedWriteHalf>>> = Arc::new(AsyncMutex::new(HashMap::new()));

    // Bridges the blocking `std::sync::mpsc::Receiver` (vmm's accept
    // thread sends into it directly, no async runtime in that crate -- see
    // `sandkiln_vmm::tunnel`'s own doc comment) into the async world via a
    // plain `spawn_blocking`, not `spawn_blocking_in_current_span`: this
    // loop outlives any single request's span, there's nothing for it to
    // meaningfully inherit.
    let (conn_tx, mut conn_rx) = tokio::sync::mpsc::unbounded_channel::<TunnelConnection>();
    tokio::task::spawn_blocking(move || {
        while let Ok(conn) = receiver.recv() {
            if conn_tx.send(conn).is_err() {
                break;
            }
        }
    });

    let new_connections = {
        let ws_sink = ws_sink.clone();
        let writers = writers.clone();
        async move {
            while let Some(conn) = conn_rx.recv().await {
                let conn_id = conn.conn_id;
                if conn.stream.set_nonblocking(true).is_err() {
                    continue;
                }
                let Ok(tokio_stream) = tokio::net::UnixStream::from_std(conn.stream) else { continue };
                let (mut read_half, write_half) = tokio_stream.into_split();
                writers.lock().await.insert(conn_id.clone(), write_half);

                if ws_sink.lock().await.send(Message::Binary(encode_frame(OP_OPEN, &conn_id, &[]))).await.is_err() {
                    break;
                }

                let ws_sink = ws_sink.clone();
                let writers = writers.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 8192];
                    loop {
                        match read_half.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if ws_sink.lock().await.send(Message::Binary(encode_frame(OP_DATA, &conn_id, &buf[..n]))).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    writers.lock().await.remove(&conn_id);
                    let _ = ws_sink.lock().await.send(Message::Binary(encode_frame(OP_CLOSE, &conn_id, &[]))).await;
                });
            }
        }
    };

    let inbound = async move {
        while let Some(Ok(msg)) = ws_stream.next().await {
            let Message::Binary(data) = msg else { continue };
            let Some((op, conn_id, payload)) = decode_frame(&data) else { continue };
            match op {
                OP_DATA => {
                    let mut guard = writers.lock().await;
                    if let Some(writer) = guard.get_mut(conn_id) {
                        if writer.write_all(payload).await.is_err() {
                            guard.remove(conn_id);
                        }
                    }
                }
                OP_CLOSE => {
                    if let Some(mut writer) = writers.lock().await.remove(conn_id) {
                        let _ = writer.shutdown().await;
                    }
                }
                _ => {}
            }
        }
    };

    tokio::join!(new_connections, inbound);
}

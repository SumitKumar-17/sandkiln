//! Host-side acceptor for guest-initiated `TUNNEL_PORT` connections — the
//! local tunnel feature (exposing a service on the caller's own machine
//! to code running inside the sandbox, the reverse of dev-server
//! preview). See `sandkiln_protocol::TUNNEL_PORT`'s own doc comment for
//! why this is the one place in the whole stack where the host listens
//! for the *guest* to connect in.
//!
//! Deliberately `std::sync::mpsc`, not a tokio channel: this crate has no
//! async runtime dependency by design (same reasoning as
//! `vsock_client.rs`'s plain blocking I/O), and the daemon already
//! bridges every vmm call through `spawn_blocking` — bridging this
//! channel the same way keeps that boundary consistent instead of
//! pulling tokio into a crate that otherwise has zero async code.

use sandkiln_protocol::{decode_tunnel_open, read_message, TunnelOpen, TUNNEL_PORT};
use std::collections::HashMap;
use std::io;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;

/// One accepted guest-initiated connection, already past its handshake —
/// the raw stream is ready to relay as-is.
pub struct TunnelConnection {
    pub conn_id: String,
    pub stream: UnixStream,
}

fn registry() -> &'static Mutex<HashMap<String, Sender<TunnelConnection>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Sender<TunnelConnection>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Registers a fresh `tunnel_id` to receive connections, returning the
/// receiving half. Call this *before* telling the guest agent to start
/// listening (`StartTunnel`) — a connection that arrives before
/// registration has nowhere to go and is dropped with a warning.
pub fn register(tunnel_id: String) -> Receiver<TunnelConnection> {
    let (tx, rx) = std::sync::mpsc::channel();
    registry().lock().unwrap().insert(tunnel_id, tx);
    rx
}

/// Removes a tunnel's registration. Not an error if it's already gone.
pub fn unregister(tunnel_id: &str) {
    registry().lock().unwrap().remove(tunnel_id);
}

/// The Unix socket path Firecracker expects a guest-initiated connection
/// on `TUNNEL_PORT` to be bridged to: the VM's own host-visible vsock UDS
/// path with `_<port>` appended, per Firecracker's guest-initiated-
/// connection convention — a *different* mechanism from the
/// `CONNECT <port>\n` handshake `vsock_client.rs` uses, which only
/// applies to the host-initiated direction.
pub fn tunnel_socket_path(vsock_socket: &Path) -> PathBuf {
    let mut s = vsock_socket.as_os_str().to_owned();
    s.push(format!("_{TUNNEL_PORT}"));
    PathBuf::from(s)
}

/// Binds the listener for one VM's `TUNNEL_PORT` traffic and accepts on a
/// background thread for the rest of the VM's life — one listener per
/// VM, shared by every tunnel that VM ever opens, dispatched by the
/// `tunnel_id` each connection's handshake names. Must be called before
/// the VM boots: Firecracker only forwards a guest connection here if
/// this socket already exists and is listening at the moment the guest
/// calls `connect()`.
pub fn listen(vsock_socket: &Path) -> io::Result<()> {
    let path = tunnel_socket_path(vsock_socket);
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;

    thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut stream) = conn else { break };
            let handshake = read_message(&mut stream).ok().and_then(|raw| decode_tunnel_open(&raw).ok());
            match handshake {
                Some(TunnelOpen { tunnel_id, conn_id }) => {
                    let sender = registry().lock().unwrap().get(&tunnel_id).cloned();
                    match sender {
                        Some(tx) => {
                            let _ = tx.send(TunnelConnection { conn_id, stream });
                        }
                        None => eprintln!("tunnel: connection for unknown or already-closed tunnel {tunnel_id}, dropping"),
                    }
                }
                None => eprintln!("tunnel: dropped a connection with a bad or missing handshake"),
            }
        }
    });
    Ok(())
}

/// Removes this VM's guest-initiated-connection socket. Best-effort,
/// same as every other per-VM teardown path in this crate — there's
/// nothing further to do if the file is already gone.
pub fn cleanup(vsock_socket: &Path) {
    let _ = std::fs::remove_file(tunnel_socket_path(vsock_socket));
}

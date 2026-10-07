//! Local tunnel: exposes a service on the *caller's own machine* to code
//! running inside the sandbox — the reverse of dev-server preview. See
//! `TUNNEL_PORT`'s own doc comment for the full design (why this is the
//! one guest-initiated vsock direction in the whole protocol).
//!
//! `start`/`stop` bind/tear down a plain TCP listener on a guest port;
//! every accepted connection gets its own thread that dials out to
//! `TUNNEL_PORT`, sends one framed `TunnelOpen` handshake, then shovels
//! bytes until either side closes — same handshake-then-passthrough
//! shape `pty.rs` uses, and the same `try_clone()`-on-one-socket hangup
//! hazard applies here too (see that file's own doc comment): dropping
//! one cloned handle doesn't close the underlying socket, so each side's
//! copy loop explicitly `shutdown(Shutdown::Both)`s on exit to unblock
//! the other rather than trusting it to notice on its own.

use sandkiln_protocol::{encode_tunnel_open, write_message, TunnelOpen, TUNNEL_PORT};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use vsock::{VsockStream, VMADDR_CID_HOST};

struct TunnelHandle {
    stop: Arc<AtomicBool>,
    port: u16,
}

fn registry() -> &'static Mutex<HashMap<String, TunnelHandle>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, TunnelHandle>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Binds a listener on `guest_port` and starts accepting connections for
/// it in the background. Returns once the bind succeeds (not once the
/// listener is actually accepting -- that loop runs on its own thread),
/// so a caller gets a real bind error (port already in use, for example)
/// synchronously rather than discovering it later.
pub fn start(tunnel_id: String, guest_port: u16) -> io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", guest_port))?;
    let stop = Arc::new(AtomicBool::new(false));
    registry().lock().unwrap().insert(tunnel_id.clone(), TunnelHandle { stop: stop.clone(), port: guest_port });

    thread::spawn(move || accept_loop(listener, tunnel_id, stop));
    Ok(())
}

/// Stops a tunnel's listener and unblocks its accept loop. A no-op, not
/// an error, if the tunnel is already gone -- the daemon may call this
/// during cleanup without knowing whether the guest side already tore it
/// down itself.
pub fn stop(tunnel_id: &str) {
    let Some(handle) = registry().lock().unwrap().remove(tunnel_id) else { return };
    handle.stop.store(true, Ordering::SeqCst);
    // `TcpListener::accept` has no non-blocking cancel in std; connecting
    // to our own listener is what actually unblocks it. The accept loop
    // recognizes this as the shutdown signal via the flag above and
    // discards the connection instead of relaying it.
    let _ = TcpStream::connect(("127.0.0.1", handle.port));
}

fn accept_loop(listener: TcpListener, tunnel_id: String, stop: Arc<AtomicBool>) {
    let conn_counter = AtomicU64::new(0);
    loop {
        let Ok((stream, _addr)) = listener.accept() else { break };
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let conn_id = format!("{tunnel_id}-{}", conn_counter.fetch_add(1, Ordering::SeqCst));
        let tid = tunnel_id.clone();
        thread::spawn(move || {
            if let Err(e) = relay(stream, tid, conn_id) {
                eprintln!("tunnel: relay error: {e}");
            }
        });
    }
}

/// Handles one accepted local connection start to finish: dials
/// `TUNNEL_PORT`, sends the handshake, shovels bytes both directions
/// until either side ends.
fn relay(local: TcpStream, tunnel_id: String, conn_id: String) -> io::Result<()> {
    let mut vsock = VsockStream::connect_with_cid_port(VMADDR_CID_HOST, TUNNEL_PORT)?;
    let payload =
        encode_tunnel_open(&TunnelOpen { tunnel_id, conn_id }).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    write_message(&mut vsock, &payload)?;

    let mut vsock_read = vsock.try_clone()?;
    let mut vsock_write = vsock;
    let mut local_read = local.try_clone()?;
    let mut local_write = local;

    // local -> vsock (the real service's request bytes, headed into the
    // sandbox-visible side of the tunnel).
    let inbound = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match local_read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) if vsock_write.write_all(&buf[..n]).is_err() => break,
                Ok(_) => {}
            }
        }
        let _ = vsock_write.shutdown(Shutdown::Both);
    });

    // vsock -> local (the sandboxed caller's traffic, headed out to the
    // real service), on the calling thread.
    let mut buf = [0u8; 8192];
    loop {
        match vsock_read.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) if local_write.write_all(&buf[..n]).is_err() => break,
            Ok(_) => {}
        }
    }
    let _ = local_write.shutdown(Shutdown::Both);

    let _ = inbound.join();
    Ok(())
}

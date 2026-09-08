//! Host-side client for talking to the guest agent over Firecracker's
//! vsock. Firecracker mediates AF_VSOCK through a plain Unix domain
//! socket: connecting to the configured UDS and sending `CONNECT <port>\n`
//! bridges the rest of that connection to the given port inside the guest.
//! See: <https://github.com/firecracker-microvm/firecracker/blob/main/docs/vsock.md>

use sandkiln_protocol::{
    decode_response, encode_pty_handshake, encode_request, read_message, write_message, CodecError, PtyHandshake, Request,
    Response,
};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// Bounds every blocking read/write on the vsock stream. Without this, a
/// guest that connects but never answers — a paused VM (vCPUs halted, so
/// the agent inside literally cannot respond) is the case that surfaced
/// this — hangs the call forever: the retry/deadline logic in
/// `crate::vm::Vm::call` only bounds *between* attempts, not a single
/// attempt that never returns at all.
const IO_TIMEOUT: Duration = Duration::from_secs(3);

/// Sends one request to the guest agent and returns its response. Opens a
/// fresh connection per call — fine for now; a persistent connection can
/// replace this later if per-call handshake overhead matters.
pub fn call(uds_path: &Path, guest_port: u32, request: &Request) -> io::Result<Response> {
    let mut stream = UnixStream::connect(uds_path)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    connect_handshake(&mut stream, guest_port)?;

    let payload = encode_request(request).map_err(to_io_err)?;
    write_message(&mut stream, &payload)?;

    let raw = read_message(&mut stream)?;
    decode_response(&raw).map_err(to_io_err)
}

/// Opens a new, long-lived vsock connection for an interactive PTY
/// session — a fundamentally different shape from `call` above: this
/// returns the raw, still-open stream for the caller to shovel bytes
/// through indefinitely, rather than reading exactly one response and
/// closing. Deliberately does *not* leave `IO_TIMEOUT` set on the
/// returned stream (unlike `call`, where every operation is a bounded
/// one-shot) — a real interactive session can go quiet for a long time
/// with nothing typed, and that's not a hung connection.
pub fn open_pty(uds_path: &Path, pty_port: u32, cols: u16, rows: u16) -> io::Result<UnixStream> {
    let mut stream = UnixStream::connect(uds_path)?;
    // Timeouts apply only to the handshake below, not to the stream
    // this function hands back — see the doc comment above.
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    connect_handshake(&mut stream, pty_port)?;

    let payload = encode_pty_handshake(&PtyHandshake { cols, rows }).map_err(to_io_err)?;
    write_message(&mut stream, &payload)?;

    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(stream)
}

fn connect_handshake(stream: &mut UnixStream, guest_port: u32) -> io::Result<()> {
    writeln!(stream, "CONNECT {guest_port}")?;

    // Firecracker replies with "OK <assigned-host-port>\n" on success.
    let mut reply = String::new();
    BufReader::new(&*stream).read_line(&mut reply)?;
    if !reply.starts_with("OK ") {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            format!("vsock connect to guest port {guest_port} failed: {}", reply.trim()),
        ));
    }
    Ok(())
}

fn to_io_err(e: CodecError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

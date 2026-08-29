//! Host-side client for talking to the guest agent over Firecracker's
//! vsock. Firecracker mediates AF_VSOCK through a plain Unix domain
//! socket: connecting to the configured UDS and sending `CONNECT <port>\n`
//! bridges the rest of that connection to the given port inside the guest.
//! See: <https://github.com/firecracker-microvm/firecracker/blob/main/docs/vsock.md>

use sandkiln_protocol::{
    decode_response, encode_request, read_message, write_message, CodecError, Request, Response,
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

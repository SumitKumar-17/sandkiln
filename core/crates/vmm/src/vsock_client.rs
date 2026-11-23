//! Host-side client for talking to the guest agent over Firecracker's
//! vsock. Firecracker mediates AF_VSOCK through a plain Unix domain
//! socket: connecting to the configured UDS and sending `CONNECT <port>\n`
//! bridges the rest of that connection to the given port inside the guest.
//! See: <https://github.com/firecracker-microvm/firecracker/blob/main/docs/vsock.md>

use sandkiln_protocol::{
    decode_response, encode_exec_stream_handshake, encode_pty_handshake, encode_request, read_message, write_message, CodecError,
    ExecStreamHandshake, PtyHandshake, Request, Response,
};
use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// Bounds every blocking read/write on the vsock stream. Without this, a
/// guest that connects but never answers (a paused VM, vCPUs halted, is
/// the case that surfaced this) hangs forever — `Vm::call`'s retry/
/// deadline logic only bounds *between* attempts, not one that never
/// returns.
const IO_TIMEOUT: Duration = Duration::from_secs(3);

/// Why a request/response call failed, and whether the guest could have
/// seen the request. The distinction is what makes retrying safe: a request
/// that never left (connect refused, handshake failed) can be sent again;
/// one that was fully written may already be running, and sending it again
/// would execute a command twice.
#[derive(Debug)]
pub struct CallError {
    pub error: io::Error,
    /// The full request frame was written to the guest. Do not retry.
    pub request_sent: bool,
}

impl From<CallError> for io::Error {
    fn from(e: CallError) -> Self {
        e.error
    }
}

/// Sends one request to the guest agent and returns its response. Opens a
/// fresh connection per call — fine for now; a persistent connection can
/// replace this later if per-call handshake overhead matters.
///
/// `response_timeout` bounds only the wait for the reply (`None` = wait
/// indefinitely); connecting and writing the request always use
/// [`IO_TIMEOUT`]. A short bound is right for control-plane calls (a paused
/// guest never answers); a user command legitimately stays silent for as
/// long as it runs, so its caller passes a much larger one.
pub fn call(uds_path: &Path, guest_port: u32, request: &Request, response_timeout: Option<Duration>) -> Result<Response, CallError> {
    let unsent = |error: io::Error| CallError { error, request_sent: false };
    let mut stream = UnixStream::connect(uds_path).map_err(unsent)?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).map_err(unsent)?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).map_err(unsent)?;
    connect_handshake(&mut stream, guest_port).map_err(unsent)?;

    let payload = encode_request(request).map_err(|e| unsent(to_io_err(e)))?;
    // A write that fails part-way leaves the agent with an incomplete frame,
    // which it discards without executing, so it still counts as unsent.
    write_message(&mut stream, &payload).map_err(unsent)?;

    let sent = |error: io::Error| CallError { error, request_sent: true };
    stream.set_read_timeout(response_timeout).map_err(sent)?;
    let raw = read_message(&mut stream).map_err(sent)?;
    decode_response(&raw).map_err(|e| sent(to_io_err(e)))
}

/// Opens a long-lived vsock connection for an interactive PTY session —
/// unlike `call`, returns the raw, still-open stream for the caller to
/// shovel bytes through indefinitely. Deliberately clears `IO_TIMEOUT` on
/// the returned stream: a real session can go quiet a long time with
/// nothing typed, and that's not a hung connection.
pub fn open_pty(uds_path: &Path, pty_port: u32, cols: u16, rows: u16) -> io::Result<UnixStream> {
    let mut stream = UnixStream::connect(uds_path)?;
    // Timeout applies only to the handshake below, not the returned stream.
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    connect_handshake(&mut stream, pty_port)?;

    let payload = encode_pty_handshake(&PtyHandshake { cols, rows }).map_err(to_io_err)?;
    write_message(&mut stream, &payload)?;

    stream.set_read_timeout(None)?;
    stream.set_write_timeout(None)?;
    Ok(stream)
}

/// Opens a long-lived vsock connection for a streamed background exec
/// session — same open-once shape as `open_pty`, but the caller reads
/// framed `ExecStreamEvent`s (`read_message` + `decode_exec_stream_event`)
/// instead of raw bytes, until `Exit` arrives and the guest closes it.
pub fn open_exec_stream(
    uds_path: &Path,
    exec_stream_port: u32,
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
) -> io::Result<UnixStream> {
    let mut stream = UnixStream::connect(uds_path)?;
    // Timeout applies only to the handshake below, same as `open_pty`.
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;
    connect_handshake(&mut stream, exec_stream_port)?;

    let payload = encode_exec_stream_handshake(&ExecStreamHandshake { command: command.to_string(), args: args.to_vec(), env: env.clone() })
        .map_err(to_io_err)?;
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

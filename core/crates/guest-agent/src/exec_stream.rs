//! Streamed background exec sessions — a fundamentally different shape
//! from `handler.rs`'s one-request-one-response exec, and from `pty.rs`'s
//! raw byte passthrough: a connection here reads one framed
//! `ExecStreamHandshake`, spawns that command with its stdout/stderr
//! piped (no controlling terminal, unlike a PTY session — this is for a
//! long-running background command, not an interactive shell), then
//! keeps sending framed `ExecStreamEvent`s back — one per chunk of
//! output, in the order produced — until the process exits, ending with
//! exactly one `Exit` before closing. See
//! `sandkiln_protocol::EXEC_STREAM_PORT`'s own doc comment for why this
//! gets a third port rather than a new `Request` variant or reusing
//! `PTY_PORT`.
//!
//! No concurrent-session cap enforced here, same reasoning as `pty.rs`:
//! that's a host-side (daemon) concern, tracked per sandbox without this
//! process needing any cross-connection state.

use base64::Engine;
use sandkiln_protocol::{decode_exec_stream_handshake, encode_exec_stream_event, read_message, write_message, ExecStreamEvent};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use vsock::VsockStream;

/// Handles one accepted [`sandkiln_protocol::EXEC_STREAM_PORT`]
/// connection start to finish. Blocks the calling thread for the whole
/// session — callers (see `main.rs`) spawn one OS thread per connection,
/// same as `pty::handle_connection`, since a background command can run
/// for a long time.
pub fn handle_connection(mut stream: VsockStream) {
    let handshake = match read_handshake(&mut stream) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("exec-stream: failed to read handshake: {e}");
            return;
        }
    };

    let mut child = match Command::new(&handshake.command)
        .args(&handshake.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("exec-stream: failed to spawn {}: {e}\n", handshake.command);
            let _ = send_event(&mut stream, &ExecStreamEvent::Stderr { data_base64: base64_encode(msg.as_bytes()) });
            let _ = send_event(&mut stream, &ExecStreamEvent::Exit { exit_code: -1 });
            return;
        }
    };

    pump_and_wait(stream, &mut child);
}

fn read_handshake(stream: &mut VsockStream) -> std::io::Result<sandkiln_protocol::ExecStreamHandshake> {
    let payload = read_message(stream)?;
    decode_exec_stream_handshake(&payload).map_err(std::io::Error::from)
}

fn send_event(stream: &mut VsockStream, event: &ExecStreamEvent) -> std::io::Result<()> {
    let payload = encode_exec_stream_event(event).map_err(std::io::Error::from)?;
    write_message(stream, &payload)
}

fn base64_encode(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

/// Reads the child's stdout and stderr concurrently (each on its own
/// thread, since either pipe can fill up and block if only read
/// sequentially), funnels both into one channel so this function's own
/// thread is the *only* one that ever writes to `stream` — avoiding any
/// need to synchronize concurrent writers, since a single thread writing
/// one message at a time can never interleave two messages' framing.
/// Waits for the child only after both pipes have hit EOF, never before
/// — waiting first risks deadlock if the child is still blocked writing
/// to a pipe nobody's draining yet.
fn pump_and_wait(mut stream: VsockStream, child: &mut Child) {
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let (tx, rx) = mpsc::channel::<ExecStreamEvent>();

    let stdout_tx = tx.clone();
    let stdout_thread = thread::spawn(move || pump_reader(stdout, stdout_tx, true));
    let stderr_thread = thread::spawn(move || pump_reader(stderr, tx, false));

    for event in rx {
        if send_event(&mut stream, &event).is_err() {
            break;
        }
    }

    let _ = stdout_thread.join();
    let _ = stderr_thread.join();

    let exit_code = match child.wait() {
        Ok(status) => status.code().unwrap_or(-1),
        Err(_) => -1,
    };
    let _ = send_event(&mut stream, &ExecStreamEvent::Exit { exit_code });
}

fn pump_reader(mut reader: impl Read, tx: mpsc::Sender<ExecStreamEvent>, is_stdout: bool) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let data_base64 = base64_encode(&buf[..n]);
                let event = if is_stdout { ExecStreamEvent::Stdout { data_base64 } } else { ExecStreamEvent::Stderr { data_base64 } };
                if tx.send(event).is_err() {
                    break;
                }
            }
        }
    }
}

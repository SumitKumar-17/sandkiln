//! Interactive PTY sessions — a fundamentally different shape from
//! `handler.rs`'s one-request-one-response exec/file operations. A
//! connection here reads exactly one framed `PtyHandshake`, then becomes
//! a raw, long-lived, bidirectional byte passthrough between the vsock
//! connection and a real pseudo-terminal running a shell. See
//! `sandkiln_protocol::PTY_PORT`'s own doc comment for the full picture.
//!
//! No concurrent-session cap is enforced here — that's deliberately a
//! host-side (daemon) concern instead, since the daemon is what actually
//! manages each session's lifecycle (a WebSocket connection) and can
//! track a cap without this process needing any cross-connection state
//! at all; see `ROADMAP.md`'s "Dev servers and live preview" section.

use nix::pty::{forkpty, ForkptyResult, Winsize};
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::waitpid;
use nix::unistd::Pid;
use sandkiln_protocol::PtyHandshake;
use std::fs::File;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::thread;
use vsock::VsockStream;

/// Handles one accepted [`sandkiln_protocol::PTY_PORT`] connection start
/// to finish: reads the handshake, forks a shell attached to a pty sized
/// to it, shovels bytes both directions until either side closes, reaps
/// the child. Blocks the calling thread for the whole session — callers
/// (see `main.rs`) spawn one OS thread per connection so a long-lived
/// PTY session never blocks anything else, the same way each `AGENT_PORT`
/// connection already gets handled to completion before the next
/// `accept()` — this just adds "on its own thread" since a PTY session,
/// unlike an exec/file-op connection, is expected to stay open for a
/// long time.
pub fn handle_connection(mut stream: VsockStream) {
    let handshake = match read_handshake(&mut stream) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("pty: failed to read handshake: {e}");
            return;
        }
    };

    let winsize = Winsize { ws_row: handshake.rows, ws_col: handshake.cols, ws_xpixel: 0, ws_ypixel: 0 };

    // SAFETY: the child branch below does nothing before `exec` except
    // resolve a shell path (a plain `Path::exists` check, no heap
    // allocation risk beyond what `Command` itself needs right before
    // exec'ing) — the same pragmatic pattern real-world Rust terminal
    // implementations (alacritty, wezterm) use around forkpty's own
    // stricter-than-enforced async-signal-safety contract.
    let fork_result = unsafe { forkpty(&winsize, None) };
    let (child, master) = match fork_result {
        Ok(ForkptyResult::Child) => exec_shell(),
        Ok(ForkptyResult::Parent { child, master }) => (child, master),
        Err(e) => {
            eprintln!("pty: forkpty failed: {e}");
            return;
        }
    };

    let master_file = File::from(master);
    shovel_bytes(stream, master_file, child);

    // Best-effort: the session is already over from the caller's
    // perspective (both copy directions have ended) by the time this
    // runs — reaping just avoids leaving a zombie process behind, it
    // doesn't block anything on the outcome.
    let _ = waitpid(child, None);
}

/// Reads the one framed message every `PTY_PORT` connection starts
/// with, using the same length-prefixed-JSON framing `AGENT_PORT`
/// connections use for every message — this is the only place a PTY
/// connection uses that framing at all; everything after this call is
/// raw bytes.
fn read_handshake(stream: &mut VsockStream) -> std::io::Result<PtyHandshake> {
    let payload = sandkiln_protocol::read_message(stream)?;
    sandkiln_protocol::decode_pty_handshake(&payload).map_err(std::io::Error::from)
}

/// Runs only inside the forked child, already attached to the pty slave
/// as stdin/stdout/stderr and already its own session leader with that
/// pty as its controlling terminal (all of that is `forkpty`'s own
/// contract, backed by glibc's `forkpty(3)`) — so this only has to pick
/// a shell and exec it. Prefers bash for a nicer interactive experience
/// (job control messages, completion, etc. — whatever the guest image's
/// own bash config provides) but never assumes it's present, since only
/// the small CI test image is guaranteed to exist; the production image
/// build always includes bash, but a caller could register a minimal
/// custom image via `POST /images` that doesn't.
fn exec_shell() -> ! {
    let shell = if std::path::Path::new("/bin/bash").exists() { "/bin/bash" } else { "/bin/sh" };
    let err = Command::new(shell).exec();
    eprintln!("pty: exec {shell} failed: {err}");
    std::process::exit(1);
}

/// Copies bytes in both directions between `stream` (the vsock
/// connection to the host) and `pty` (the pty master fd) until either
/// side reaches EOF or errors, then actively unblocks the other
/// direction instead of trusting it to notice on its own — it often
/// can't: `stream_read`/`stream_write` are `try_clone()`d handles to the
/// *same* underlying vsock socket, so dropping just one of them on
/// thread exit doesn't close the connection (the kernel keeps it open
/// as long as any fd still references it) and leaves the other thread
/// blocked in `read()` forever, waiting for bytes nobody is left to
/// send. Two cases:
///   - the shell exits first (pty EOF) -- shut the vsock socket down so
///     the blocked vsock-input read on the other thread returns immediately;
///   - the host disconnects first (vsock EOF) -- send the shell's
///     process group SIGHUP, the same signal a real terminal sends on
///     hangup, so it actually terminates instead of running orphaned.
fn shovel_bytes(stream: VsockStream, pty: File, child: Pid) {
    let mut stream_read = match stream.try_clone() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("pty: failed to clone vsock stream: {e}");
            return;
        }
    };
    let mut stream_write = stream;
    let mut pty_read = match pty.try_clone() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("pty: failed to clone pty fd: {e}");
            return;
        }
    };
    let mut pty_write = pty;

    // pty output -> vsock (the shell's own stdout/stderr, interleaved,
    // exactly as a real terminal would see it).
    let output_thread = thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match pty_read.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if stream_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = stream_write.shutdown(Shutdown::Both);
    });

    // vsock input -> pty (keystrokes/pasted input from whoever opened
    // this session), on the calling thread.
    let mut buf = [0u8; 8192];
    loop {
        match stream_read.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if pty_write.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
    let _ = kill(child, Signal::SIGHUP);

    let _ = output_thread.join();
}

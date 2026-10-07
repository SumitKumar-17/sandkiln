//! Interactive PTY sessions — unlike `handler.rs`'s one-request-one-
//! response ops, a connection here reads one framed `PtyHandshake` then
//! becomes a raw, long-lived, bidirectional passthrough between the
//! vsock connection and a real pty running a shell. See `PTY_PORT`'s doc
//! comment for the full picture.
//!
//! No concurrent-session cap here — deliberately a host-side (daemon)
//! concern, since it already manages each session's WebSocket lifecycle
//! and can track a cap with no cross-connection state needed here.

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

/// Handles one accepted `PTY_PORT` connection start to finish: reads the
/// handshake, forks a shell attached to a sized pty, shovels bytes both
/// directions until either side closes, reaps the child. Blocks the
/// calling thread for the whole session — `main.rs` spawns one OS thread
/// per connection since a PTY session is expected to stay open a long time.
pub fn handle_connection(mut stream: VsockStream) {
    let handshake = match read_handshake(&mut stream) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("pty: failed to read handshake: {e}");
            return;
        }
    };

    let winsize = Winsize { ws_row: handshake.rows, ws_col: handshake.cols, ws_xpixel: 0, ws_ypixel: 0 };

    // SAFETY: the child branch does nothing before `exec` except resolve
    // a shell path (`Path::exists`, no risky heap allocation) — the same
    // pragmatic pattern alacritty/wezterm use around forkpty's stricter-
    // than-enforced async-signal-safety contract.
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

    // Best-effort: the session's already over by the time this runs —
    // just avoids leaving a zombie behind.
    let _ = waitpid(child, None);
}

/// Reads the one framed message a `PTY_PORT` connection starts with
/// (same length-prefixed-JSON framing `AGENT_PORT` uses) — the only
/// place this connection uses framing at all; everything after is raw
/// bytes.
fn read_handshake(stream: &mut VsockStream) -> std::io::Result<PtyHandshake> {
    let payload = sandkiln_protocol::read_message(stream)?;
    sandkiln_protocol::decode_pty_handshake(&payload).map_err(std::io::Error::from)
}

/// Runs only in the forked child, already attached to the pty slave and
/// already session leader with it as controlling terminal (`forkpty`'s
/// own contract) — just picks a shell and execs it. Prefers bash but
/// never assumes it's present, since a registered custom image
/// (`POST /images`) might not include it.
fn exec_shell() -> ! {
    let shell = if std::path::Path::new("/bin/bash").exists() { "/bin/bash" } else { "/bin/sh" };
    let err = Command::new(shell).exec();
    eprintln!("pty: exec {shell} failed: {err}");
    std::process::exit(1);
}

/// Copies bytes both directions between `stream` and `pty` until either
/// side hits EOF/errors, then actively unblocks the other direction
/// rather than trusting it to notice. It often can't:
/// `stream_read`/`stream_write` are `try_clone()`d handles to the *same*
/// socket, so dropping one on thread exit doesn't close the connection
/// (the kernel keeps it open while any fd references it), leaving the
/// other thread blocked in `read()` forever. Two cases: shell exits first
/// (pty EOF) → shut the vsock socket down so the blocked input read
/// returns immediately; host disconnects first (vsock EOF) → SIGHUP the
/// shell's process group, same as a real terminal hangup, so it
/// terminates instead of running orphaned.
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

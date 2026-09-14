mod exec_stream;
mod handler;
mod pty;

use sandkiln_protocol::{decode_request, encode_response, read_message, write_message, Response, AGENT_PORT, EXEC_STREAM_PORT, PTY_PORT};
use std::io::{Read, Write};
use vsock::{VsockListener, VMADDR_CID_ANY};

fn main() {
    // Each its own thread, concurrent with the exec/file-op loop below
    // and with each other — PTY sessions and streamed exec sessions are
    // both long-lived (a real interactive shell, or a long-running
    // background command, can stay open indefinitely) and, unlike that
    // loop's one-connection-at-a-time design, more than one may
    // legitimately be open at once (see `pty`'s module doc comment on
    // why the concurrent-session cap lives on the host side instead of
    // here — the same reasoning applies to `exec_stream`).
    std::thread::spawn(run_pty_listener);
    std::thread::spawn(run_exec_stream_listener);

    let listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, AGENT_PORT).expect("bind vsock listener");

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => handle_connection(stream),
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
}

fn run_pty_listener() {
    let listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, PTY_PORT).expect("bind pty vsock listener");
    for conn in listener.incoming() {
        match conn {
            // One thread per connection, not handled inline like the
            // exec/file-op loop above — a PTY session blocks for as
            // long as the shell it wraps stays open, so accepting the
            // next session can't wait for this one to end.
            Ok(stream) => {
                std::thread::spawn(move || pty::handle_connection(stream));
            }
            Err(e) => eprintln!("pty accept error: {e}"),
        }
    }
}

fn run_exec_stream_listener() {
    let listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, EXEC_STREAM_PORT).expect("bind exec-stream vsock listener");
    for conn in listener.incoming() {
        match conn {
            // One thread per connection, same reasoning as the PTY
            // listener above — a background command can run for a long
            // time, so accepting the next session can't wait for this
            // one to finish.
            Ok(stream) => {
                std::thread::spawn(move || exec_stream::handle_connection(stream));
            }
            Err(e) => eprintln!("exec-stream accept error: {e}"),
        }
    }
}

fn handle_connection(mut stream: impl Read + Write) {
    while let Ok(raw) = read_message(&mut stream) {
        let response = match decode_request(&raw) {
            Ok(req) => handler::handle(req),
            Err(e) => Response::Error { message: format!("bad request: {e}") },
        };

        let payload = match encode_response(&response) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("failed to serialize response: {e}");
                break;
            }
        };

        if write_message(&mut stream, &payload).is_err() {
            break;
        }
    }
}

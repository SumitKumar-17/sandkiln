mod handler;

use sandkiln_protocol::{decode_request, encode_response, read_message, write_message, Response, AGENT_PORT};
use std::io::{Read, Write};
use vsock::{VsockListener, VMADDR_CID_ANY};

fn main() {
    let listener =
        VsockListener::bind_with_cid_port(VMADDR_CID_ANY, AGENT_PORT).expect("bind vsock listener");

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => handle_connection(stream),
            Err(e) => eprintln!("accept error: {e}"),
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

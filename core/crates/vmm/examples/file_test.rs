//! Manual end-to-end check for the vsock file ops: writes a file into the
//! guest, reads it back, and lists its parent directory.
//!
//! Usage: file_test <uds-path> <guest-port> <path>

use base64::Engine;
use sandkiln_protocol::{Request, Response};
use sandkiln_vmm::vsock_client;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(uds_path), Some(guest_port), Some(path)) = (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: file_test <uds-path> <guest-port> <path>");
        return ExitCode::FAILURE;
    };
    let uds_path = PathBuf::from(uds_path);
    let guest_port: u32 = guest_port.parse().expect("guest-port must be a number");

    let content = b"written by sandkiln file_test\n";
    let content_base64 = base64::engine::general_purpose::STANDARD.encode(content);

    let write = Request::WriteFile { path: path.clone(), content_base64 };
    match vsock_client::call(&uds_path, guest_port, &write) {
        Ok(Response::Ok) => println!("write ok"),
        other => {
            eprintln!("write failed: {other:?}");
            return ExitCode::FAILURE;
        }
    }

    let read = Request::ReadFile { path: path.clone() };
    match vsock_client::call(&uds_path, guest_port, &read) {
        Ok(Response::File { content_base64 }) => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(&content_base64).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            let ok = bytes == content;
            println!("read back: {text:?} (matches: {ok})");
            if !ok {
                return ExitCode::FAILURE;
            }
        }
        other => {
            eprintln!("read failed: {other:?}");
            return ExitCode::FAILURE;
        }
    }

    let parent = Path::new(&path).parent().unwrap_or(Path::new("/")).to_string_lossy().into_owned();
    let list = Request::ListDir { path: parent };
    match vsock_client::call(&uds_path, guest_port, &list) {
        Ok(Response::Dir { entries }) => {
            println!("dir listing has {} entries", entries.len());
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("list_dir failed: {other:?}");
            ExitCode::FAILURE
        }
    }
}

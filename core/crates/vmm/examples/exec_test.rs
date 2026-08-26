//! Manual end-to-end check for the vsock exec path: connects to a running
//! guest agent through Firecracker's vsock UDS and runs one command.
//!
//! Usage: exec_test <uds-path> <guest-port> <command> [args...]

use sandkiln_protocol::{Request, Response};
use sandkiln_vmm::vsock_client;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(uds_path), Some(guest_port), Some(command)) = (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: exec_test <uds-path> <guest-port> <command> [args...]");
        return ExitCode::FAILURE;
    };
    let guest_port: u32 = guest_port.parse().expect("guest-port must be a number");
    let command_args: Vec<String> = args.collect();

    let request = Request::Exec { command, args: command_args };
    match vsock_client::call(&PathBuf::from(uds_path), guest_port, &request) {
        Ok(Response::Exec { stdout, stderr, exit_code }) => {
            print!("{stdout}");
            eprint!("{stderr}");
            if exit_code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Ok(other) => {
            eprintln!("unexpected response: {other:?}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("vsock call failed: {e}");
            ExitCode::FAILURE
        }
    }
}

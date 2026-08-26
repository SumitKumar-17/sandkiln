use base64::Engine;
use sandkiln_protocol::{Request, Response};
use std::process::Command;

pub fn handle(req: Request) -> Response {
    match req {
        Request::Exec { command, args } => exec(&command, &args),
        Request::ReadFile { path } => read_file(&path),
        Request::WriteFile { path, content_base64 } => write_file(&path, &content_base64),
        Request::ListDir { path } => list_dir(&path),
    }
}

fn exec(command: &str, args: &[String]) -> Response {
    match Command::new(command).args(args).output() {
        Ok(out) => Response::Exec {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            exit_code: out.status.code().unwrap_or(-1),
        },
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn read_file(path: &str) -> Response {
    match std::fs::read(path) {
        Ok(bytes) => Response::File {
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        },
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn write_file(path: &str, content_base64: &str) -> Response {
    let bytes = match base64::engine::general_purpose::STANDARD.decode(content_base64) {
        Ok(b) => b,
        Err(e) => return Response::Error { message: e.to_string() },
    };
    match std::fs::write(path, bytes) {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn list_dir(path: &str) -> Response {
    match std::fs::read_dir(path) {
        Ok(rd) => {
            let entries = rd
                .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
                .collect();
            Response::Dir { entries }
        }
        Err(e) => Response::Error { message: e.to_string() },
    }
}

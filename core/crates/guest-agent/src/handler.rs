use base64::Engine;
use sandkiln_protocol::{DirEntry, Request, Response};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;

pub fn handle(req: Request) -> Response {
    match req {
        Request::Exec { command, args } => exec(&command, &args),
        Request::ReadFile { path } => read_file(&path),
        Request::WriteFile { path, content_base64 } => write_file(&path, &content_base64),
        Request::ListDir { path } => list_dir(&path),
        Request::Chmod { path, mode } => chmod(&path, mode),
        Request::Chown { path, uid, gid } => chown(&path, uid, gid),
        Request::Mkdir { path, parents } => mkdir(&path, parents),
        Request::Rename { from, to } => rename(&from, &to),
        Request::Copy { from, to } => copy(&from, &to),
        Request::Symlink { target, link_path } => symlink(&target, &link_path),
        Request::Readlink { path } => readlink(&path),
        Request::Truncate { path, size } => truncate(&path, size),
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

/// Permission bits only (e.g. `0o644`) — masks off the file-type bits
/// `st_mode`/`Metadata::mode()` also carries, so this matches exactly
/// what `Chmod::mode` takes as input (round-trippable: list a dir, take
/// an entry's `mode`, hand it straight to `Chmod`).
fn permission_bits(mode: u32) -> u32 {
    mode & 0o7777
}

fn list_dir(path: &str) -> Response {
    match std::fs::read_dir(path) {
        Ok(rd) => {
            let entries = rd
                .filter_map(|entry| entry.ok())
                .map(|entry| {
                    let metadata = entry.metadata();
                    let is_symlink = entry.file_type().map(|ft| ft.is_symlink()).unwrap_or(false);
                    let (is_dir, size, mode, mtime_unix) = match &metadata {
                        Ok(m) => (
                            m.is_dir(),
                            m.len(),
                            permission_bits(m.mode()),
                            m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0),
                        ),
                        // A dir entry that vanished or became unreadable between
                        // the readdir and the stat -- report it with zeroed
                        // metadata rather than dropping it silently, so the
                        // caller at least sees the name existed.
                        Err(_) => (false, 0, 0, 0),
                    };
                    DirEntry { name: entry.file_name().to_string_lossy().into_owned(), is_dir, is_symlink, size, mode, mtime_unix }
                })
                .collect();
            Response::Dir { entries }
        }
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn chmod(path: &str, mode: u32) -> Response {
    match std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)) {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn chown(path: &str, uid: u32, gid: u32) -> Response {
    let c_path = match std::ffi::CString::new(path) {
        Ok(c) => c,
        Err(e) => return Response::Error { message: e.to_string() },
    };
    // SAFETY: c_path is a valid, NUL-terminated C string for the
    // duration of this call, and chown(2) has no other safety
    // preconditions beyond a valid pointer.
    let result = unsafe { libc::chown(c_path.as_ptr(), uid, gid) };
    if result == 0 {
        Response::Ok
    } else {
        Response::Error { message: std::io::Error::last_os_error().to_string() }
    }
}

fn mkdir(path: &str, parents: bool) -> Response {
    let result = if parents { std::fs::create_dir_all(path) } else { std::fs::create_dir(path) };
    match result {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn rename(from: &str, to: &str) -> Response {
    match std::fs::rename(from, to) {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn copy(from: &str, to: &str) -> Response {
    match std::fs::copy(from, to) {
        Ok(_bytes_copied) => Response::Ok,
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn symlink(target: &str, link_path: &str) -> Response {
    match std::os::unix::fs::symlink(target, link_path) {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn readlink(path: &str) -> Response {
    match std::fs::read_link(path) {
        Ok(target) => Response::Link { target: target.to_string_lossy().into_owned() },
        Err(e) => Response::Error { message: e.to_string() },
    }
}

fn truncate(path: &str, size: u64) -> Response {
    match std::fs::OpenOptions::new().write(true).open(path).and_then(|f| f.set_len(size)) {
        Ok(()) => Response::Ok,
        Err(e) => Response::Error { message: e.to_string() },
    }
}

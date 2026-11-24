use crate::bounded_exec::{self, Outcome};
use base64::Engine;
use sandkiln_protocol::{DirEntry, Request, Response};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;
use std::time::Duration;

pub fn handle(req: Request) -> Response {
    match req {
        Request::Exec { command, args, env, timeout_secs: None } => exec(&command, &args, &env),
        Request::Exec { command, args, env, timeout_secs: Some(secs) } => exec_bounded(&command, &args, &env, secs),
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
        Request::StartTunnel { tunnel_id, guest_port } => match crate::tunnel::start(tunnel_id, guest_port) {
            Ok(()) => Response::Ok,
            Err(e) => Response::Error { message: e.to_string() },
        },
        Request::ResumeFixup { unix_secs, unix_nanos, entropy_base64 } => {
            match crate::fixup::apply(unix_secs, unix_nanos, &entropy_base64) {
                Ok(()) => Response::Ok,
                Err(e) => Response::Error { message: e },
            }
        }
        Request::StopTunnel { tunnel_id } => {
            crate::tunnel::stop(&tunnel_id);
            Response::Ok
        }
    }
}

/// Shell convention for a command that could not be started: 127 when it
/// does not exist, 126 when it exists but cannot be executed. Returns the exit
/// code and the line a shell would print, or `None` for any other error (out of
/// memory, too many processes, ...): those are real failures of the agent's
/// own, not a verdict on the command, and stay `Response::Error`.
pub(crate) fn spawn_failure(command: &str, err: &std::io::Error) -> Option<(i32, String)> {
    let (code, reason) = match (err.kind(), err.raw_os_error()) {
        (std::io::ErrorKind::NotFound, _) if !command.contains('/') => (127, "command not found".to_string()),
        (std::io::ErrorKind::NotFound, _) | (_, Some(libc::ENOTDIR)) => (127, "No such file or directory".to_string()),
        // exec(2) on a directory fails with EACCES; a shell says what is wrong.
        (std::io::ErrorKind::PermissionDenied, _) if std::path::Path::new(command).is_dir() => (126, "Is a directory".to_string()),
        (std::io::ErrorKind::PermissionDenied, _) => (126, "Permission denied".to_string()),
        (_, Some(libc::ENOEXEC)) => (126, "Exec format error".to_string()),
        _ => return None,
    };
    Some((code, format!("{command}: {reason}\n")))
}

/// Maps a failed spawn to a normal exec result where the convention applies.
fn spawn_failed(command: &str, err: std::io::Error) -> Response {
    match spawn_failure(command, &err) {
        Some((exit_code, stderr)) => Response::Exec { stdout: String::new(), stderr, exit_code },
        None => Response::Error { message: err.to_string() },
    }
}

fn exec(command: &str, args: &[String], env: &std::collections::HashMap<String, String>) -> Response {
    match Command::new(command).args(args).envs(env).output() {
        Ok(out) => Response::Exec {
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            exit_code: out.status.code().unwrap_or(-1),
        },
        Err(e) => spawn_failed(command, e),
    }
}

fn exec_bounded(command: &str, args: &[String], env: &std::collections::HashMap<String, String>, timeout_secs: u64) -> Response {
    match bounded_exec::run(command, args, env, Duration::from_secs(timeout_secs)) {
        Ok(Outcome::Finished { stdout, stderr, status }) => Response::Exec {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            exit_code: status.code().unwrap_or(-1),
        },
        Ok(Outcome::TimedOut { stdout, stderr }) => Response::TimedOut {
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        },
        Ok(Outcome::SpawnFailed(e)) => spawn_failed(command, e),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::os::unix::fs::PermissionsExt;

    /// Runs `command` through the handler. A file a test wrote moments ago can fail
    /// to exec with ETXTBSY ("Text file busy") when a concurrent test thread forks
    /// while this one still holds the write handle (the child keeps a copy until it
    /// execs); the kernel clears it within milliseconds and the agent correctly
    /// reports it as an error, not 126, so the retry lives in the test helper only.
    fn exec_result(command: &str, timeout_secs: Option<u64>) -> (String, String, i32) {
        for _ in 0..50 {
            let req = Request::Exec { command: command.to_string(), args: vec![], env: HashMap::new(), timeout_secs };
            match handle(req) {
                Response::Exec { stdout, stderr, exit_code } => return (stdout, stderr, exit_code),
                Response::Error { message } if message.contains("Text file busy") => std::thread::sleep(std::time::Duration::from_millis(20)),
                other => panic!("expected Exec, got {other:?}"),
            }
        }
        panic!("{command} stayed busy for a second")
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sk-handler-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_command_is_127_with_a_shell_style_message() {
        for timeout in [None, Some(5)] {
            let (stdout, stderr, code) = exec_result("sk-no-such-command", timeout);
            assert_eq!((stdout.as_str(), code), ("", 127), "timeout {timeout:?}");
            assert_eq!(stderr, "sk-no-such-command: command not found\n");
        }
    }

    #[test]
    fn missing_path_is_127_naming_the_path() {
        for timeout in [None, Some(5)] {
            let (_, stderr, code) = exec_result("/definitely/not/here", timeout);
            assert_eq!(code, 127);
            assert_eq!(stderr, "/definitely/not/here: No such file or directory\n");
        }
    }

    #[test]
    fn path_through_a_non_directory_is_127() {
        let (_, _, code) = exec_result("/etc/passwd/child", None);
        assert_eq!(code, 127);
    }

    #[test]
    fn directory_is_126() {
        for timeout in [None, Some(5)] {
            let (_, stderr, code) = exec_result("/tmp", timeout);
            assert_eq!(code, 126);
            assert_eq!(stderr, "/tmp: Is a directory\n");
        }
    }

    #[test]
    fn non_executable_file_is_126() {
        let dir = scratch("noexec");
        let file = dir.join("data.txt");
        std::fs::write(&file, "echo hi\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        for timeout in [None, Some(5)] {
            let (_, stderr, code) = exec_result(file.to_str().unwrap(), timeout);
            assert_eq!(code, 126, "timeout {timeout:?}");
            assert!(stderr.ends_with(": Permission denied\n"), "{stderr}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn executable_that_is_not_a_valid_binary_is_126() {
        let dir = scratch("badfmt");
        let file = dir.join("bad");
        std::fs::write(&file, [0u8, 1, 2, 3, 4, 5, 6, 7]).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (_, stderr, code) = exec_result(file.to_str().unwrap(), None);
        assert_eq!(code, 126);
        assert!(stderr.ends_with(": Exec format error\n"), "{stderr}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_script_whose_interpreter_is_missing_is_127() {
        let dir = scratch("shebang");
        let file = dir.join("script");
        std::fs::write(&file, "#!/no/such/interpreter\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (_, _, code) = exec_result(file.to_str().unwrap(), None);
        assert_eq!(code, 127);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_real_exit_code_127_from_a_running_command_is_untouched() {
        let req = Request::Exec { command: "sh".into(), args: vec!["-c".into(), "echo out; echo err >&2; exit 127".into()], env: HashMap::new(), timeout_secs: None };
        let Response::Exec { stdout, stderr, exit_code } = handle(req) else { panic!("expected Exec") };
        assert_eq!((stdout.as_str(), stderr.as_str(), exit_code), ("out\n", "err\n", 127));
    }

    #[test]
    fn other_spawn_errors_stay_errors() {
        let oom = std::io::Error::from_raw_os_error(libc::ENOMEM);
        assert_eq!(spawn_failure("x", &oom), None);
        let again = std::io::Error::from_raw_os_error(libc::EAGAIN);
        assert_eq!(spawn_failure("x", &again), None);
    }

    #[test]
    fn nul_byte_in_the_command_stays_an_error() {
        // Not a verdict on a command that exists or not: the request is malformed.
        let req = Request::Exec { command: "a\0b".into(), args: vec![], env: HashMap::new(), timeout_secs: None };
        assert!(matches!(handle(req), Response::Error { .. }));
    }
}

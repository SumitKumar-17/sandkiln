use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Exec {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    ReadFile {
        path: String,
    },
    WriteFile {
        path: String,
        content_base64: String,
    },
    ListDir {
        path: String,
    },
    /// `mode` is the raw permission bits (e.g. `0o644`), same as the
    /// argument to POSIX `chmod(2)` — not a symbolic string like the
    /// `chmod` shell command accepts.
    Chmod {
        path: String,
        mode: u32,
    },
    Chown {
        path: String,
        uid: u32,
        gid: u32,
    },
    /// `parents: true` behaves like `mkdir -p` (creates missing parent
    /// directories, succeeds if the target already exists); `false`
    /// (the default) behaves like plain `mkdir` — fails if the parent is
    /// missing or the target already exists.
    Mkdir {
        path: String,
        #[serde(default)]
        parents: bool,
    },
    Rename {
        from: String,
        to: String,
    },
    /// A full byte-for-byte copy to a new path — `from` is left
    /// untouched, unlike `Rename`.
    Copy {
        from: String,
        to: String,
    },
    Symlink {
        target: String,
        link_path: String,
    },
    Readlink {
        path: String,
    },
    Truncate {
        path: String,
        size: u64,
    },
}

/// One entry from a `ListDir` response — enough metadata to distinguish
/// files/dirs/symlinks and their size/permissions/mtime without a
/// separate round trip per entry.
#[derive(Debug, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: u64,
    /// Permission bits only (e.g. `0o644`), same shape `Chmod::mode` takes.
    pub mode: u32,
    pub mtime_unix: u64,
}

/// The one framed message sent at the start of a [`crate::PTY_PORT`]
/// connection, before it becomes a raw byte passthrough — see that
/// constant's own doc comment for the full picture. Not part of the
/// `Request`/`Response` enums: a PTY connection is a different protocol
/// entirely, not a new operation on the existing one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PtyHandshake {
    pub cols: u16,
    pub rows: u16,
}

/// The one framed message sent at the start of a
/// [`crate::EXEC_STREAM_PORT`] connection — same role as [`PtyHandshake`]
/// on [`crate::PTY_PORT`], but naming the command to run instead of a
/// terminal size. See that constant's own doc comment for why this is a
/// third, separate connection shape rather than a new `Request` variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecStreamHandshake {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// One framed message in the sequence an [`crate::EXEC_STREAM_PORT`]
/// connection sends after the handshake. Unlike [`crate::PTY_PORT`],
/// framing never stops here — each chunk of the spawned process's own
/// stdout/stderr becomes one of these, in the order produced, ending
/// with exactly one `Exit` right before the guest closes the connection.
/// `data_base64` (not raw bytes) for the same reason `WriteFile`/`File`
/// already encode file content that way: this whole protocol is framed
/// JSON, and process output isn't guaranteed to be valid UTF-8.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "stream", rename_all = "snake_case")]
pub enum ExecStreamEvent {
    Stdout { data_base64: String },
    Stderr { data_base64: String },
    Exit { exit_code: i32 },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    Exec {
        stdout: String,
        stderr: String,
        exit_code: i32,
    },
    File {
        content_base64: String,
    },
    Dir {
        entries: Vec<DirEntry>,
    },
    /// The result of `Readlink` — the target a symlink points at,
    /// exactly as stored (not resolved/canonicalized).
    Link {
        target: String,
    },
    Ok,
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    // These pin the exact wire shape both sides of the protocol depend
    // on — a passing roundtrip test alone wouldn't catch an accidental
    // rename of the tag field or a variant that silently changed shape,
    // since serde would happily round-trip the *new* shape through
    // itself. Fixed expected JSON catches that.

    #[test]
    fn exec_request_wire_shape() {
        let req = Request::Exec { command: "echo".to_string(), args: vec!["hi".to_string()] };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"exec","command":"echo","args":["hi"]}"#);
    }

    #[test]
    fn exec_request_args_default_to_empty_when_omitted() {
        let req: Request = serde_json::from_str(r#"{"cmd":"exec","command":"echo"}"#).unwrap();
        let Request::Exec { command, args } = req else { panic!("expected Exec") };
        assert_eq!(command, "echo");
        assert!(args.is_empty());
    }

    #[test]
    fn read_file_request_wire_shape() {
        let req = Request::ReadFile { path: "/tmp/x".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"read_file","path":"/tmp/x"}"#);
    }

    #[test]
    fn write_file_request_wire_shape() {
        let req = Request::WriteFile { path: "/tmp/x".to_string(), content_base64: "aGk=".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"write_file","path":"/tmp/x","content_base64":"aGk="}"#);
    }

    #[test]
    fn list_dir_request_wire_shape() {
        let req = Request::ListDir { path: "/tmp".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"list_dir","path":"/tmp"}"#);
    }

    #[test]
    fn chmod_request_wire_shape() {
        let req = Request::Chmod { path: "/tmp/x".to_string(), mode: 0o644 };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"chmod","path":"/tmp/x","mode":420}"#);
    }

    #[test]
    fn chown_request_wire_shape() {
        let req = Request::Chown { path: "/tmp/x".to_string(), uid: 1000, gid: 1000 };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"chown","path":"/tmp/x","uid":1000,"gid":1000}"#);
    }

    #[test]
    fn mkdir_request_wire_shape() {
        let req = Request::Mkdir { path: "/tmp/x".to_string(), parents: true };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"mkdir","path":"/tmp/x","parents":true}"#);
    }

    #[test]
    fn mkdir_request_parents_defaults_to_false_when_omitted() {
        let req: Request = serde_json::from_str(r#"{"cmd":"mkdir","path":"/tmp/x"}"#).unwrap();
        let Request::Mkdir { path, parents } = req else { panic!("expected Mkdir") };
        assert_eq!(path, "/tmp/x");
        assert!(!parents);
    }

    #[test]
    fn rename_request_wire_shape() {
        let req = Request::Rename { from: "/a".to_string(), to: "/b".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"rename","from":"/a","to":"/b"}"#);
    }

    #[test]
    fn copy_request_wire_shape() {
        let req = Request::Copy { from: "/a".to_string(), to: "/b".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"copy","from":"/a","to":"/b"}"#);
    }

    #[test]
    fn symlink_request_wire_shape() {
        let req = Request::Symlink { target: "/a".to_string(), link_path: "/b".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"symlink","target":"/a","link_path":"/b"}"#);
    }

    #[test]
    fn readlink_request_wire_shape() {
        let req = Request::Readlink { path: "/a".to_string() };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"readlink","path":"/a"}"#);
    }

    #[test]
    fn truncate_request_wire_shape() {
        let req = Request::Truncate { path: "/a".to_string(), size: 1024 };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(json, r#"{"cmd":"truncate","path":"/a","size":1024}"#);
    }

    #[test]
    fn exec_response_wire_shape() {
        let resp = Response::Exec { stdout: "out".to_string(), stderr: "err".to_string(), exit_code: 1 };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"status":"exec","stdout":"out","stderr":"err","exit_code":1}"#);
    }

    #[test]
    fn dir_response_wire_shape() {
        let resp = Response::Dir {
            entries: vec![DirEntry {
                name: "a.txt".to_string(),
                is_dir: false,
                is_symlink: false,
                size: 12,
                mode: 0o644,
                mtime_unix: 1700000000,
            }],
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(
            json,
            r#"{"status":"dir","entries":[{"name":"a.txt","is_dir":false,"is_symlink":false,"size":12,"mode":420,"mtime_unix":1700000000}]}"#
        );
    }

    #[test]
    fn link_response_wire_shape() {
        let resp = Response::Link { target: "/a".to_string() };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"status":"link","target":"/a"}"#);
    }

    #[test]
    fn ok_response_wire_shape() {
        assert_eq!(serde_json::to_string(&Response::Ok).unwrap(), r#"{"status":"ok"}"#);
    }

    #[test]
    fn error_response_wire_shape() {
        let resp = Response::Error { message: "boom".to_string() };
        assert_eq!(serde_json::to_string(&resp).unwrap(), r#"{"status":"error","message":"boom"}"#);
    }

    #[test]
    fn pty_handshake_wire_shape() {
        let handshake = PtyHandshake { cols: 80, rows: 24 };
        let json = serde_json::to_string(&handshake).unwrap();
        assert_eq!(json, r#"{"cols":80,"rows":24}"#);
    }

    #[test]
    fn exec_stream_handshake_wire_shape() {
        let handshake = ExecStreamHandshake { command: "tail".to_string(), args: vec!["-f".to_string(), "/log".to_string()] };
        let json = serde_json::to_string(&handshake).unwrap();
        assert_eq!(json, r#"{"command":"tail","args":["-f","/log"]}"#);
    }

    #[test]
    fn exec_stream_handshake_args_default_to_empty_when_omitted() {
        let handshake: ExecStreamHandshake = serde_json::from_str(r#"{"command":"tail"}"#).unwrap();
        assert_eq!(handshake.command, "tail");
        assert!(handshake.args.is_empty());
    }

    #[test]
    fn exec_stream_event_wire_shapes() {
        let stdout = ExecStreamEvent::Stdout { data_base64: "aGk=".to_string() };
        assert_eq!(serde_json::to_string(&stdout).unwrap(), r#"{"stream":"stdout","data_base64":"aGk="}"#);

        let stderr = ExecStreamEvent::Stderr { data_base64: "b29wcw==".to_string() };
        assert_eq!(serde_json::to_string(&stderr).unwrap(), r#"{"stream":"stderr","data_base64":"b29wcw=="}"#);

        let exit = ExecStreamEvent::Exit { exit_code: 0 };
        assert_eq!(serde_json::to_string(&exit).unwrap(), r#"{"stream":"exit","exit_code":0}"#);
    }

    #[test]
    fn every_request_variant_roundtrips() {
        let requests = [
            Request::Exec { command: "ls".to_string(), args: vec!["-la".to_string()] },
            Request::ReadFile { path: "/a".to_string() },
            Request::WriteFile { path: "/a".to_string(), content_base64: "x".to_string() },
            Request::ListDir { path: "/a".to_string() },
            Request::Chmod { path: "/a".to_string(), mode: 0o644 },
            Request::Chown { path: "/a".to_string(), uid: 0, gid: 0 },
            Request::Mkdir { path: "/a".to_string(), parents: true },
            Request::Rename { from: "/a".to_string(), to: "/b".to_string() },
            Request::Copy { from: "/a".to_string(), to: "/b".to_string() },
            Request::Symlink { target: "/a".to_string(), link_path: "/b".to_string() },
            Request::Readlink { path: "/a".to_string() },
            Request::Truncate { path: "/a".to_string(), size: 0 },
        ];
        for req in requests {
            let json = serde_json::to_vec(&req).unwrap();
            let back: Request = serde_json::from_slice(&json).unwrap();
            // Request has no PartialEq (it wraps plain data, not worth
            // deriving just for this) — comparing the re-serialized form
            // is an equally strong roundtrip check.
            assert_eq!(serde_json::to_vec(&back).unwrap(), json);
        }
    }
}

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

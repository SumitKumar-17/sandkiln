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
        entries: Vec<String>,
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
    fn exec_response_wire_shape() {
        let resp = Response::Exec { stdout: "out".to_string(), stderr: "err".to_string(), exit_code: 1 };
        let json = serde_json::to_string(&resp).unwrap();
        assert_eq!(json, r#"{"status":"exec","stdout":"out","stderr":"err","exit_code":1}"#);
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

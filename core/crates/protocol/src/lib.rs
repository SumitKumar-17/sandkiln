//! The wire protocol shared between the guest agent (runs inside a
//! microVM, speaks this over vsock) and the host side (the vmm crate,
//! speaks this over the vsock UDS). Kept dependency-light and separate
//! from both so neither side has to depend on the other's internals.

mod framing;
mod messages;

pub use framing::{read_message, write_message};
pub use messages::{Request, Response};

/// The vsock port the guest agent listens on, and the host connects to.
/// Lives here so the two sides can't drift out of sync on it.
pub const AGENT_PORT: u32 = 5000;

/// The wire encoding (JSON) is an implementation detail; this alias lets
/// callers handle codec errors without depending on serde_json directly.
pub type CodecError = serde_json::Error;

/// Decodes a `Request` from a message payload (as produced by
/// `read_message`). The wire encoding (JSON) is an implementation detail
/// callers shouldn't need to know about.
pub fn decode_request(payload: &[u8]) -> Result<Request, CodecError> {
    serde_json::from_slice(payload)
}

/// Encodes a `Response` into a message payload (to pass to
/// `write_message`).
pub fn encode_response(response: &Response) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(response)
}

/// Encodes a `Request` into a message payload — used by the host-side
/// client.
pub fn encode_request(request: &Request) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(request)
}

/// Decodes a `Response` from a message payload — used by the host-side
/// client.
pub fn decode_response(payload: &[u8]) -> Result<Response, CodecError> {
    serde_json::from_slice(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Exercises the exact pipeline a real request actually goes
    /// through: encode -> frame -> (network) -> unframe -> decode. Each
    /// piece has its own unit tests (`framing.rs`, `messages.rs`); this
    /// is the one test proving they compose correctly together.
    #[test]
    fn full_request_pipeline_host_to_guest() {
        let request = Request::Exec { command: "echo".to_string(), args: vec!["hi".to_string()] };

        let payload = encode_request(&request).unwrap();
        let mut wire = Vec::new();
        write_message(&mut wire, &payload).unwrap();

        let received_payload = read_message(&mut Cursor::new(wire)).unwrap();
        let decoded = decode_request(&received_payload).unwrap();

        let Request::Exec { command, args } = decoded else { panic!("expected Exec") };
        assert_eq!(command, "echo");
        assert_eq!(args, vec!["hi"]);
    }

    #[test]
    fn full_response_pipeline_guest_to_host() {
        let response = Response::Exec { stdout: "hi\n".to_string(), stderr: String::new(), exit_code: 0 };

        let payload = encode_response(&response).unwrap();
        let mut wire = Vec::new();
        write_message(&mut wire, &payload).unwrap();

        let received_payload = read_message(&mut Cursor::new(wire)).unwrap();
        let decoded = decode_response(&received_payload).unwrap();

        let Response::Exec { stdout, exit_code, .. } = decoded else { panic!("expected Exec") };
        assert_eq!(stdout, "hi\n");
        assert_eq!(exit_code, 0);
    }

    #[test]
    fn decode_request_rejects_malformed_json() {
        assert!(decode_request(b"not json").is_err());
    }

    #[test]
    fn decode_request_rejects_unknown_cmd_tag() {
        assert!(decode_request(br#"{"cmd":"reboot_the_host"}"#).is_err());
    }
}

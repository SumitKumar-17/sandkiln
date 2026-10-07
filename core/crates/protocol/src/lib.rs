//! The wire protocol shared between the guest agent (runs inside a
//! microVM, speaks this over vsock) and the host side (the vmm crate,
//! speaks this over the vsock UDS). Kept dependency-light and separate
//! from both so neither side has to depend on the other's internals.

mod framing;
mod messages;

pub use framing::{read_message, write_message};
pub use messages::{DirEntry, ExecStreamEvent, ExecStreamHandshake, PtyHandshake, Request, Response};

/// The vsock port the guest agent listens on for exec/file-op requests,
/// host-connected. Lives here so the two sides can't drift out of sync.
pub const AGENT_PORT: u32 = 5000;

/// A second port for interactive PTY sessions — one long-lived, raw
/// bidirectional byte stream, unlike `AGENT_PORT`'s one-request-one-
/// response-then-close, so it gets its own port rather than a new
/// `Request`/`Response` variant. Sends exactly one framed
/// [`PtyHandshake`], then becomes a raw passthrough: every byte after
/// that goes straight to the pty, no more framing.
pub const PTY_PORT: u32 = 5001;

/// A third port for streamed background exec sessions (`kiln logs -f`).
/// Like `PTY_PORT`, one long-lived connection per session; unlike it,
/// never drops to raw bytes — every message after the initial
/// [`ExecStreamHandshake`] stays framed JSON ([`ExecStreamEvent`]),
/// since a caller needs structured output (which stream, and the exit
/// code), not an undifferentiated byte stream.
pub const EXEC_STREAM_PORT: u32 = 5002;

/// The wire encoding (JSON) is an implementation detail; this alias lets
/// callers handle codec errors without depending on serde_json directly.
pub type CodecError = serde_json::Error;

/// Decodes a `Request` from a `read_message` payload — guest agent side.
pub fn decode_request(payload: &[u8]) -> Result<Request, CodecError> {
    serde_json::from_slice(payload)
}

/// Encodes a `Response` for `write_message` — guest agent side.
pub fn encode_response(response: &Response) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(response)
}

/// Encodes a `Request` — host-side client.
pub fn encode_request(request: &Request) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(request)
}

/// Decodes a `Response` — host-side client.
pub fn decode_response(payload: &[u8]) -> Result<Response, CodecError> {
    serde_json::from_slice(payload)
}

/// Encodes a [`PTY_PORT`] connection's opening handshake — host-side client.
pub fn encode_pty_handshake(handshake: &PtyHandshake) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(handshake)
}

/// Decodes a [`PtyHandshake`] — guest agent's `PTY_PORT` handler.
pub fn decode_pty_handshake(payload: &[u8]) -> Result<PtyHandshake, CodecError> {
    serde_json::from_slice(payload)
}

/// Encodes an [`EXEC_STREAM_PORT`] connection's opening handshake —
/// host-side client.
pub fn encode_exec_stream_handshake(handshake: &ExecStreamHandshake) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(handshake)
}

/// Decodes an [`ExecStreamHandshake`] — guest agent's `EXEC_STREAM_PORT` handler.
pub fn decode_exec_stream_handshake(payload: &[u8]) -> Result<ExecStreamHandshake, CodecError> {
    serde_json::from_slice(payload)
}

/// Encodes one [`ExecStreamEvent`] — guest agent streaming output back.
pub fn encode_exec_stream_event(event: &ExecStreamEvent) -> Result<Vec<u8>, CodecError> {
    serde_json::to_vec(event)
}

/// Decodes an [`ExecStreamEvent`] — host-side client reading the stream.
pub fn decode_exec_stream_event(payload: &[u8]) -> Result<ExecStreamEvent, CodecError> {
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
        let request =
            Request::Exec { command: "echo".to_string(), args: vec!["hi".to_string()], env: std::collections::HashMap::new() };

        let payload = encode_request(&request).unwrap();
        let mut wire = Vec::new();
        write_message(&mut wire, &payload).unwrap();

        let received_payload = read_message(&mut Cursor::new(wire)).unwrap();
        let decoded = decode_request(&received_payload).unwrap();

        let Request::Exec { command, args, .. } = decoded else { panic!("expected Exec") };
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

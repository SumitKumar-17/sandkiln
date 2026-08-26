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

//! Correlates one HTTP request with every VM operation it triggers.
//!
//! [`correlate`] is wired as the outermost layer on `app` in `main.rs`. It
//! resolves a request id (accepting a caller-supplied `X-Request-Id` header
//! when present and well-formed, generating a UUID otherwise — see
//! [`resolve`]), echoes it back on the response, and wraps the rest of the
//! request's handling in a `tracing::Span` carrying that id. Every
//! `#[tracing::instrument]`ed route handler this wraps creates its span as a
//! child of that one (span parenting follows "current span" at creation
//! time), and `tracing_subscriber`'s default formatters (pretty and JSON
//! alike) render a event's full span ancestry, so `request_id` shows up on
//! every log line for the request — including ones emitted from
//! `sandkiln-vmm` deep inside a `spawn_blocking` closure, via
//! `tracing_util::spawn_blocking_in_current_span`.

use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;
use uuid::Uuid;

const REQUEST_HEADER: &str = "x-request-id";
const RESPONSE_HEADER: HeaderName = HeaderName::from_static("x-request-id");
/// Generous enough for any real caller-supplied id (UUIDs are 36 chars;
/// some tracing systems use longer opaque ids) while keeping a hostile
/// caller from writing an unbounded string into every log line for the
/// life of the request.
const MAX_LEN: usize = 200;

/// Wraps the rest of request handling in a span carrying the resolved
/// request id, and echoes that id back as `X-Request-Id` on the response.
pub async fn correlate(request: Request, next: Next) -> Response {
    let request_id = resolve(request.headers());
    let span = tracing::info_span!("http_request", request_id = %request_id);

    async move {
        let mut response = next.run(request).await;
        // `resolve` only ever returns strings `is_valid_request_id` already
        // accepted (ASCII printable, length-bounded) or a freshly generated
        // UUID, both of which are always valid header values — this can't
        // actually fail, but a response missing its own correlation header
        // is a strange thing to panic the request over, so fail open.
        if let Ok(value) = HeaderValue::from_str(&request_id) {
            response.headers_mut().insert(RESPONSE_HEADER, value);
        }
        response
    }
    .instrument(span)
    .await
}

/// Uses the caller-supplied `X-Request-Id` header if present and
/// well-formed, so a request can be correlated with a caller's own
/// upstream trace id (e.g. a reverse proxy or an orchestrating service);
/// generates a fresh UUID otherwise, so every request is still correlated
/// even from a caller that doesn't set one.
fn resolve(headers: &axum::http::HeaderMap) -> String {
    headers
        .get(REQUEST_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| is_valid_request_id(value))
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

/// Rejects the empty string, anything unreasonably long, and anything
/// containing whitespace or control characters — a caller-supplied id
/// gets written verbatim into every log line and the response header for
/// this request, so it's validated at the boundary rather than trusted.
fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_LEN && value.chars().all(|c| c.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn headers_with(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(REQUEST_HEADER, HeaderValue::from_str(value).unwrap());
        headers
    }

    #[test]
    fn is_valid_request_id_accepts_a_uuid() {
        assert!(is_valid_request_id("f47ac10b-58cc-4372-a567-0e02b2c3d479"));
    }

    #[test]
    fn is_valid_request_id_accepts_an_opaque_upstream_trace_id() {
        assert!(is_valid_request_id("abc123:span-7"));
    }

    #[test]
    fn is_valid_request_id_rejects_empty() {
        assert!(!is_valid_request_id(""));
    }

    #[test]
    fn is_valid_request_id_rejects_whitespace() {
        assert!(!is_valid_request_id("has space"));
        assert!(!is_valid_request_id("tab\tchar"));
    }

    #[test]
    fn is_valid_request_id_rejects_control_characters() {
        assert!(!is_valid_request_id("evil\nline\ninjection"));
        assert!(!is_valid_request_id("evil\r\nSet-Cookie: x=y"));
    }

    #[test]
    fn is_valid_request_id_rejects_unreasonably_long_values() {
        let long = "a".repeat(MAX_LEN + 1);
        assert!(!is_valid_request_id(&long));
    }

    #[test]
    fn is_valid_request_id_accepts_exactly_the_max_length() {
        let max = "a".repeat(MAX_LEN);
        assert!(is_valid_request_id(&max));
    }

    #[test]
    fn resolve_uses_a_well_formed_caller_supplied_header() {
        let headers = headers_with("caller-trace-42");
        assert_eq!(resolve(&headers), "caller-trace-42");
    }

    #[test]
    fn resolve_trims_surrounding_whitespace() {
        let headers = headers_with("  caller-trace-42  ");
        assert_eq!(resolve(&headers), "caller-trace-42");
    }

    #[test]
    fn resolve_generates_a_uuid_when_the_header_is_absent() {
        let id = resolve(&HeaderMap::new());
        assert!(Uuid::parse_str(&id).is_ok(), "expected a UUID, got {id}");
    }

    #[test]
    fn resolve_generates_a_uuid_when_the_header_is_invalid() {
        let headers = headers_with("has space");
        let id = resolve(&headers);
        assert!(Uuid::parse_str(&id).is_ok(), "expected a fallback UUID, got {id}");
    }

    #[test]
    fn resolve_generates_a_different_uuid_each_call_when_no_header_is_given() {
        assert_ne!(resolve(&HeaderMap::new()), resolve(&HeaderMap::new()));
    }
}

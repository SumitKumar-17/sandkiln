//! Reverse proxy for a sandbox's own network port: `/sandboxes/:id/preview/:port[/*path]`
//! forwards the full HTTP request (method, headers, body) to
//! `http://<sandbox guest ip>:<port>/<path>` over the bridge network
//! (`sandkiln_vmm::network::Lease::config.guest_ip`) and streams the
//! response straight back, so a dev server running inside a sandbox can be
//! reached from a normal browser.
//!
//! Not wrapped in `auth::require_bearer_token` like the rest of
//! `/sandboxes*` — see `auth::require_preview_token` for why (in short: a
//! browser navigating straight to this URL, or embedding it in an
//! `<iframe>`, can't attach an `Authorization` header, so this route's own
//! auth middleware also accepts the token as a `?token=` query parameter).
//! That token, and the daemon's own `Authorization` header, are exactly
//! the two things `request_headers_to_forward` strips before anything
//! reaches the guest — the guest is running untrusted/AI-generated code
//! and must never see this API's credential.

use crate::error::AppError;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderName, Uri};
use axum::response::Response;
use hyper_util::client::legacy::connect::HttpConnector;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Instant;

pub type PreviewClient = hyper_util::client::legacy::Client<HttpConnector, axum::body::Body>;

/// No sub-path given (`/sandboxes/:id/preview/:port`, no trailing
/// segment) — a separate route from `preview_path` below because axum's
/// `*rest` wildcard only matches when there's at least a trailing `/`,
/// not the bare prefix.
pub async fn preview_root(
    State(state): State<Arc<AppState>>,
    Path((id, port)): Path<(String, String)>,
    req: axum::extract::Request,
) -> Result<Response, AppError> {
    proxy(state, id, port, String::new(), req).await
}

pub async fn preview_path(
    State(state): State<Arc<AppState>>,
    Path((id, port, path)): Path<(String, String, String)>,
    req: axum::extract::Request,
) -> Result<Response, AppError> {
    proxy(state, id, port, path, req).await
}

async fn proxy(
    state: Arc<AppState>,
    id: String,
    port_str: String,
    tail_path: String,
    req: axum::extract::Request,
) -> Result<Response, AppError> {
    let port: u16 = port_str.parse().map_err(|_| AppError::BadRequest(format!("invalid preview port: {port_str}")))?;

    let guest_ip = {
        let sandboxes = state.sandboxes.lock().unwrap();
        let sandbox = sandboxes.get(&id).ok_or_else(|| AppError::NotFound(id.clone()))?;
        // A live preview session is exactly the kind of activity the idle
        // reaper should count — a browser sitting on a dev-server tab with
        // no `exec` traffic shouldn't get the sandbox stopped out from
        // under it.
        *sandbox.last_activity.lock().unwrap() = Instant::now();

        // A sandbox forked from a snapshot (`source_snapshot_id.is_some()`)
        // doesn't own a `Lease` of its own — the snapshot still holds it
        // (see `routes_snapshot`'s module doc comment) — but the VM it
        // booted still has a real, working network interface: the guest's
        // IP/MAC were baked into the snapshotted memory image at the
        // *original* boot and are unaffected by which `Sandbox` record the
        // daemon happens to track the `Lease` under. Look it up via the
        // snapshot instead of assuming every sandbox owns its own lease.
        match &sandbox.network {
            Some(lease) => lease.config.guest_ip,
            None => {
                let source_id = sandbox.source_snapshot_id.as_ref().ok_or_else(|| {
                    AppError::Internal(std::io::Error::other(format!(
                        "sandbox {id} has no network lease and no source snapshot — this is a bug"
                    )))
                })?;
                let snapshots = state.snapshots.lock().unwrap();
                let snapshot = snapshots.get(source_id).ok_or_else(|| {
                    AppError::Internal(std::io::Error::other(format!(
                        "sandbox {id} was forked from snapshot {source_id}, which no longer exists — this is a bug"
                    )))
                })?;
                snapshot.network.config.guest_ip
            }
        }
    };

    let (parts, body) = req.into_parts();
    let query = strip_token_param(parts.uri.query().unwrap_or(""));
    let target = build_target_uri(guest_ip, port, &tail_path, query.as_deref())
        .map_err(|e| AppError::BadRequest(format!("could not build preview target for {id}: {e}")))?;

    let mut forwarded = axum::http::Request::builder()
        .method(parts.method)
        .uri(target)
        .body(body)
        .map_err(|e| AppError::BadRequest(format!("invalid preview request: {e}")))?;
    *forwarded.headers_mut() = request_headers_to_forward(&parts.headers);

    let response = tokio::time::timeout(state.config.preview_timeout, state.preview_client.request(forwarded))
        .await
        .map_err(|_| {
            AppError::GatewayTimeout(format!("sandbox {id} port {port} did not respond within the preview timeout"))
        })?
        .map_err(|e| {
            AppError::BadGateway(format!("sandbox {id} port {port} is not reachable: {e}"))
        })?;

    let (mut resp_parts, incoming) = response.into_parts();
    resp_parts.headers = response_headers_to_forward(&resp_parts.headers);
    Ok(Response::from_parts(resp_parts, axum::body::Body::new(incoming)))
}

/// Hop-by-hop headers per RFC 7230 §6.1 — meaningful only to the specific
/// TCP connection carrying them, never something to relay from one leg of
/// a proxy to the other.
const HOP_BY_HOP_HEADERS: &[&str] =
    &["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "te", "trailers", "transfer-encoding", "upgrade"];

fn is_hop_by_hop(name: &HeaderName) -> bool {
    HOP_BY_HOP_HEADERS.iter().any(|h| name.as_str().eq_ignore_ascii_case(h))
}

/// Headers forwarded from the caller to the guest's dev server. Beyond the
/// generic hop-by-hop set: `host` is dropped so the outgoing request gets
/// one derived from the actual proxy target instead of the daemon's own
/// hostname, and `authorization` is dropped unconditionally — it may carry
/// this API's own `SANDKILN_AUTH_TOKEN` (see `auth::require_preview_token`),
/// which must never reach the sandboxed guest process.
fn request_headers_to_forward(original: &HeaderMap) -> HeaderMap {
    original
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name) && **name != header::HOST && **name != header::AUTHORIZATION)
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// Headers forwarded from the guest's response back to the caller —
/// everything except the generic hop-by-hop set. Unlike the request
/// direction there's nothing daemon-specific to strip: the guest never
/// saw the daemon's own credential, so nothing it sends back could echo it.
fn response_headers_to_forward(original: &HeaderMap) -> HeaderMap {
    original.iter().filter(|(name, _)| !is_hop_by_hop(name)).map(|(name, value)| (name.clone(), value.clone())).collect()
}

/// Builds `http://<guest_ip>:<port><tail_path>[?query]`. `tail_path` is
/// axum's wildcard capture, which never includes the leading `/` — this is
/// the one place that gets normalized back in.
fn build_target_uri(
    guest_ip: Ipv4Addr,
    port: u16,
    tail_path: &str,
    query: Option<&str>,
) -> Result<Uri, axum::http::uri::InvalidUri> {
    let path = match tail_path {
        "" => "/".to_string(),
        p if p.starts_with('/') => p.to_string(),
        p => format!("/{p}"),
    };
    let mut uri_string = format!("http://{guest_ip}:{port}{path}");
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        uri_string.push('?');
        uri_string.push_str(q);
    }
    uri_string.parse()
}

/// The reserved query parameter a preview URL carries this API's auth
/// token in, for callers (a browser navigating directly) that can't set an
/// `Authorization` header. Reserved because it's stripped before the
/// request ever reaches the guest — see `request_headers_to_forward`'s doc
/// comment for why forwarding it would be a real credential leak, and this
/// is the query-string half of the same concern.
pub(crate) const TOKEN_QUERY_PARAM: &str = "token";

/// Finds the reserved `token` query parameter's raw value, if present.
/// Deliberately does not percent-decode: this API's own tokens are opaque
/// operator-configured strings, expected to contain only URL-safe
/// characters, so a raw byte comparison against the configured token is
/// both correct for the expected case and avoids round-tripping the rest
/// of the query string through a decode/re-encode that could change bytes
/// a dev server depends on (see `strip_token_param`).
pub(crate) fn find_token_param(query: &str) -> Option<&str> {
    query_segments(query).find_map(|segment| raw_param_value(segment, TOKEN_QUERY_PARAM))
}

/// Removes the reserved `token` parameter from a raw query string,
/// returning `None` if nothing is left. Operates on raw, still
/// percent-encoded segments and rejoins them as-is, rather than
/// decoding/re-encoding the whole query string, so every other parameter's
/// exact original bytes reach the guest unchanged.
fn strip_token_param(query: &str) -> Option<String> {
    let remaining: Vec<&str> = query_segments(query).filter(|segment| raw_param_value(segment, TOKEN_QUERY_PARAM).is_none()).collect();
    if remaining.is_empty() {
        None
    } else {
        Some(remaining.join("&"))
    }
}

fn query_segments(query: &str) -> impl Iterator<Item = &str> {
    query.split('&').filter(|s| !s.is_empty())
}

/// `segment` is one raw `key` or `key=value` piece of a query string
/// (already split on `&`). Returns the value if `segment`'s key matches
/// `key` exactly — `""` for a bare `key` with no `=`.
fn raw_param_value<'a>(segment: &'a str, key: &str) -> Option<&'a str> {
    let rest = segment.strip_prefix(key)?;
    match rest.strip_prefix('=') {
        Some(value) => Some(value),
        None if rest.is_empty() => Some(""),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn build_target_uri_root_path_becomes_slash() {
        let uri = build_target_uri("172.16.0.5".parse().unwrap(), 3000, "", None).unwrap();
        assert_eq!(uri.to_string(), "http://172.16.0.5:3000/");
    }

    #[test]
    fn build_target_uri_preserves_sub_path_without_leading_slash() {
        let uri = build_target_uri("172.16.0.5".parse().unwrap(), 3000, "api/users", None).unwrap();
        assert_eq!(uri.to_string(), "http://172.16.0.5:3000/api/users");
    }

    #[test]
    fn build_target_uri_preserves_sub_path_with_leading_slash() {
        let uri = build_target_uri("172.16.0.5".parse().unwrap(), 3000, "/api/users", None).unwrap();
        assert_eq!(uri.to_string(), "http://172.16.0.5:3000/api/users");
    }

    #[test]
    fn build_target_uri_appends_query_string() {
        let uri = build_target_uri("172.16.0.5".parse().unwrap(), 3000, "search", Some("q=rust")).unwrap();
        assert_eq!(uri.to_string(), "http://172.16.0.5:3000/search?q=rust");
    }

    #[test]
    fn build_target_uri_omits_empty_query() {
        let uri = build_target_uri("172.16.0.5".parse().unwrap(), 3000, "search", Some("")).unwrap();
        assert_eq!(uri.to_string(), "http://172.16.0.5:3000/search");
    }

    #[test]
    fn find_token_param_reads_the_value() {
        assert_eq!(find_token_param("token=secret123"), Some("secret123"));
        assert_eq!(find_token_param("a=1&token=secret123&b=2"), Some("secret123"));
    }

    #[test]
    fn find_token_param_absent_is_none() {
        assert_eq!(find_token_param("a=1&b=2"), None);
        assert_eq!(find_token_param(""), None);
    }

    #[test]
    fn find_token_param_does_not_match_a_longer_key() {
        assert_eq!(find_token_param("tokenized=1"), None);
    }

    #[test]
    fn find_token_param_bare_key_with_no_value_is_empty_string() {
        assert_eq!(find_token_param("token"), Some(""));
    }

    #[test]
    fn strip_token_param_removes_only_the_token_param() {
        assert_eq!(strip_token_param("a=1&token=secret&b=2"), Some("a=1&b=2".to_string()));
    }

    #[test]
    fn strip_token_param_leaving_nothing_is_none() {
        assert_eq!(strip_token_param("token=secret"), None);
    }

    #[test]
    fn strip_token_param_no_token_present_is_unchanged() {
        assert_eq!(strip_token_param("a=1&b=2"), Some("a=1&b=2".to_string()));
    }

    #[test]
    fn strip_token_param_empty_query_is_none() {
        assert_eq!(strip_token_param(""), None);
    }

    #[test]
    fn request_headers_strips_hop_by_hop_host_and_authorization() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("daemon.internal"));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer secret"));
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(header::ACCEPT, HeaderValue::from_static("text/html"));
        headers.insert("x-custom", HeaderValue::from_static("value"));

        let forwarded = request_headers_to_forward(&headers);
        assert!(!forwarded.contains_key(header::HOST));
        assert!(!forwarded.contains_key(header::AUTHORIZATION));
        assert!(!forwarded.contains_key(header::CONNECTION));
        assert_eq!(forwarded.get(header::ACCEPT).unwrap(), "text/html");
        assert_eq!(forwarded.get("x-custom").unwrap(), "value");
    }

    #[test]
    fn response_headers_strip_hop_by_hop_but_keep_everything_else() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(header::TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
        headers.insert(header::SET_COOKIE, HeaderValue::from_static("session=abc"));

        let forwarded = response_headers_to_forward(&headers);
        assert!(!forwarded.contains_key(header::CONNECTION));
        assert!(!forwarded.contains_key(header::TRANSFER_ENCODING));
        assert_eq!(forwarded.get(header::CONTENT_TYPE).unwrap(), "text/html");
        assert_eq!(forwarded.get(header::SET_COOKIE).unwrap(), "session=abc");
    }
}

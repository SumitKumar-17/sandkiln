use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

/// No-op when `auth_token` is unset (local dev). Otherwise requires
/// `Authorization: Bearer <token>` matching it exactly.
pub async fn require_bearer_token(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = &state.config.auth_token else {
        return Ok(next.run(req).await);
    };

    let provided = req.headers().get(header::AUTHORIZATION).and_then(|value| value.to_str().ok());
    if token_matches(provided, expected) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Pure comparison, pulled out of the axum-specific plumbing above so it's
/// directly testable without constructing a full `AppState`/`Request`.
/// `header_value` is the raw `Authorization` header value, if present.
fn token_matches(header_value: Option<&str>, expected: &str) -> bool {
    header_value.and_then(|value| value.strip_prefix("Bearer ")).is_some_and(|provided| constant_time_eq(provided, expected))
}

/// Compares secrets without an early exit on the first differing byte, so
/// response timing does not reveal how much of a guessed token was right.
/// Runs over the longer input; only the length itself can leak.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

/// Guards `/sandboxes/:id/preview/*` — deliberately not `require_bearer_token`
/// above. A preview URL is meant to be opened directly by a browser (a tab
/// navigation, or an `<iframe src=...>` embedding a sandbox's dev server),
/// neither of which can attach a custom `Authorization` header, so this
/// additionally accepts the token via the reserved `?token=` query
/// parameter `routes::preview::find_token_param` reads. Still a no-op when
/// `SANDKILN_AUTH_TOKEN` is unset, same as `require_bearer_token`.
pub async fn require_preview_token(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = &state.config.auth_token else {
        return Ok(next.run(req).await);
    };

    let header_value = req.headers().get(header::AUTHORIZATION).and_then(|value| value.to_str().ok());
    let query_token = req.uri().query().and_then(crate::http::routes::preview::find_token_param);
    if preview_token_matches(header_value, query_token, expected) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Pure version of `require_preview_token`'s check, mirroring
/// `token_matches` above.
fn preview_token_matches(header_value: Option<&str>, query_token: Option<&str>, expected: &str) -> bool {
    token_matches(header_value, expected) || query_token.is_some_and(|t| constant_time_eq(t, expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_correct_bearer_token() {
        assert!(token_matches(Some("Bearer secret123"), "secret123"));
    }

    #[test]
    fn rejects_a_wrong_token() {
        assert!(!token_matches(Some("Bearer wrong"), "secret123"));
    }

    #[test]
    fn rejects_a_missing_header() {
        assert!(!token_matches(None, "secret123"));
    }

    #[test]
    fn rejects_the_bare_token_without_the_bearer_prefix() {
        assert!(!token_matches(Some("secret123"), "secret123"));
    }

    #[test]
    fn rejects_wrong_auth_scheme() {
        assert!(!token_matches(Some("Basic secret123"), "secret123"));
    }

    #[test]
    fn is_case_sensitive_and_exact() {
        assert!(!token_matches(Some("Bearer Secret123"), "secret123"));
        assert!(!token_matches(Some("Bearer secret123 "), "secret123"));
        assert!(!token_matches(Some("Bearer secret12"), "secret123"));
    }

    #[test]
    fn preview_token_matches_via_header_alone() {
        assert!(preview_token_matches(Some("Bearer secret123"), None, "secret123"));
    }

    #[test]
    fn preview_token_matches_via_query_alone() {
        assert!(preview_token_matches(None, Some("secret123"), "secret123"));
    }

    #[test]
    fn preview_token_rejects_wrong_query_token() {
        assert!(!preview_token_matches(None, Some("wrong"), "secret123"));
    }

    #[test]
    fn preview_token_rejects_when_neither_is_present() {
        assert!(!preview_token_matches(None, None, "secret123"));
    }

    #[test]
    fn constant_time_eq_agrees_with_plain_equality() {
        assert!(constant_time_eq("", ""));
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
        assert!(!constant_time_eq("abcd", "abc"));
        assert!(!constant_time_eq("", "a"));
    }
}

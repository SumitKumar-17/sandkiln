use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub enum AppError {
    NotFound(String),
    DriveNotFound(String),
    ImageNotFound(String),
    BadRequest(String),
    /// The request is well-formed but conflicts with current state — e.g.
    /// deleting or attaching a drive that's already attached elsewhere.
    Conflict(String),
    /// A `/preview` proxy request reached a sandbox but not the target
    /// port inside it — connection refused (nothing listening), reset, or
    /// otherwise unreachable at the TCP level. Distinct from `Internal`
    /// since this is never the daemon's own fault: it's exactly what a
    /// browser sees hitting a dead port through any reverse proxy.
    BadGateway(String),
    /// A `/preview` proxy request's guest side didn't respond within
    /// `Config::preview_timeout`.
    GatewayTimeout(String),
    Internal(std::io::Error),
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Internal(e)
    }
}

impl AppError {
    /// Shared by `IntoResponse` (paired with the status code) and
    /// `Display` (message alone, for `tracing::warn!(error = %e, ...)` call
    /// sites like `idle_reaper`'s auto-suspend-failure logging, which has
    /// no HTTP response to shape) — one place defining what each variant's
    /// message text actually is, rather than two copies drifting apart.
    fn status_and_message(&self) -> (StatusCode, String) {
        match self {
            AppError::NotFound(id) => (StatusCode::NOT_FOUND, format!("sandbox not found: {id}")),
            AppError::DriveNotFound(id) => (StatusCode::NOT_FOUND, format!("drive not found: {id}")),
            AppError::ImageNotFound(id) => (StatusCode::NOT_FOUND, format!("image not found: {id}")),
            AppError::BadRequest(message) => (StatusCode::BAD_REQUEST, message.clone()),
            AppError::Conflict(message) => (StatusCode::CONFLICT, message.clone()),
            AppError::BadGateway(message) => (StatusCode::BAD_GATEWAY, message.clone()),
            AppError::GatewayTimeout(message) => (StatusCode::GATEWAY_TIMEOUT, message.clone()),
            AppError::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.status_and_message().1)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = self.status_and_message();
        (status, Json(json!({ "error": message }))).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    /// Every caller of this API depends on `{"error": "<message>"}` as
    /// the error body shape — these pin both the status code and that
    /// exact shape per variant, not just "it returns a response".
    async fn status_and_message(response: Response) -> (StatusCode, String) {
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        (status, body["error"].as_str().unwrap().to_string())
    }

    #[tokio::test]
    async fn not_found_is_404_with_sandbox_wording() {
        let (status, message) = status_and_message(AppError::NotFound("abc".to_string()).into_response()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(message, "sandbox not found: abc");
    }

    #[tokio::test]
    async fn drive_not_found_is_404_with_drive_wording() {
        let (status, message) = status_and_message(AppError::DriveNotFound("d1".to_string()).into_response()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(message, "drive not found: d1");
    }

    #[tokio::test]
    async fn image_not_found_is_404_with_image_wording() {
        let (status, message) = status_and_message(AppError::ImageNotFound("img1".to_string()).into_response()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(message, "image not found: img1");
    }

    #[tokio::test]
    async fn bad_request_is_400_with_the_given_message_verbatim() {
        let (status, message) = status_and_message(AppError::BadRequest("bad input".to_string()).into_response()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(message, "bad input");
    }

    #[tokio::test]
    async fn conflict_is_409_with_the_given_message_verbatim() {
        let (status, message) = status_and_message(AppError::Conflict("already attached".to_string()).into_response()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(message, "already attached");
    }

    #[tokio::test]
    async fn bad_gateway_is_502_with_the_given_message_verbatim() {
        let (status, message) = status_and_message(AppError::BadGateway("connection refused".to_string()).into_response()).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(message, "connection refused");
    }

    #[tokio::test]
    async fn gateway_timeout_is_504_with_the_given_message_verbatim() {
        let (status, message) =
            status_and_message(AppError::GatewayTimeout("no response in time".to_string()).into_response()).await;
        assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(message, "no response in time");
    }

    #[tokio::test]
    async fn internal_is_500_and_never_leaks_beyond_the_io_error_text() {
        let io_err = std::io::Error::other("disk on fire");
        let (status, message) = status_and_message(AppError::Internal(io_err).into_response()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(message, "disk on fire");
    }

    #[tokio::test]
    async fn io_error_converts_to_internal_via_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        let app_err: AppError = io_err.into();
        let (status, _) = status_and_message(app_err.into_response()).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    }
}

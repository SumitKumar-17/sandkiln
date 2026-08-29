use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub enum AppError {
    NotFound(String),
    DriveNotFound(String),
    BadRequest(String),
    /// The request is well-formed but conflicts with current state — e.g.
    /// deleting or attaching a drive that's already attached elsewhere.
    Conflict(String),
    Internal(std::io::Error),
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Internal(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::NotFound(id) => (StatusCode::NOT_FOUND, format!("sandbox not found: {id}")),
            AppError::DriveNotFound(id) => (StatusCode::NOT_FOUND, format!("drive not found: {id}")),
            AppError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            AppError::Conflict(message) => (StatusCode::CONFLICT, message),
            AppError::Internal(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

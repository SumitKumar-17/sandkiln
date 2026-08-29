//! `/metrics`: Prometheus text-exposition-format output. Unauthenticated
//! like `/healthz` — this is operational data about the daemon itself,
//! not sandbox data, so it doesn't need to sit behind the bearer token
//! that gates `/sandboxes*`.

use crate::state::AppState;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use std::sync::Arc;

pub async fn metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let sandboxes_active = state.sandboxes.lock().unwrap().len();
    let body = state.metrics.render(sandboxes_active);
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}

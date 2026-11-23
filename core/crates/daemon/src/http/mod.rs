//! The HTTP surface: one router assembled from the per-resource modules in
//! `routes`, with the auth layer chosen per route group (bearer token for
//! the REST API, a `?token=` fallback for browser-reachable preview and
//! WebSocket routes) and request-id correlation outermost.

pub mod auth;
pub mod error;
pub mod request_id;
pub mod routes;

use crate::state::AppState;
use axum::middleware;
use axum::routing::{any, delete, get, post};
use axum::Router;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

pub fn build_router(state: Arc<AppState>) -> Router {
    let sandbox_routes = Router::new()
        .route("/sandboxes", post(routes::sandbox::create_sandbox).get(routes::sandbox::list_sandboxes))
        .route("/sandboxes/history", get(routes::sandbox::sandbox_history))
        .route("/sandboxes/get-or-create", post(routes::sandbox_name::get_or_create_sandbox))
        .route("/sandboxes/by-name/:name", get(routes::sandbox_name::get_sandbox_by_name))
        .route("/sandboxes/:id", delete(routes::sandbox::stop_sandbox))
        .route("/sandboxes/:id/exec", post(routes::exec::exec))
        .route("/sandboxes/:id/read-file", post(routes::exec::read_file))
        .route("/sandboxes/:id/write-file", post(routes::exec::write_file))
        .route("/sandboxes/:id/chmod", post(routes::fs::chmod))
        .route("/sandboxes/:id/chown", post(routes::fs::chown))
        .route("/sandboxes/:id/mkdir", post(routes::fs::mkdir))
        .route("/sandboxes/:id/rename", post(routes::fs::rename))
        .route("/sandboxes/:id/copy", post(routes::fs::copy))
        .route("/sandboxes/:id/symlink", post(routes::fs::symlink))
        .route("/sandboxes/:id/readlink", post(routes::fs::readlink))
        .route("/sandboxes/:id/truncate", post(routes::fs::truncate))
        .route("/sandboxes/:id/list-dir", post(routes::fs::list_dir))
        .route("/sandboxes/:id/exec-stream", post(routes::logs::start_exec_stream).get(routes::logs::list_exec_streams))
        .merge(routes::mounts::router())
        .merge(routes::tunnel::router())
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    // Its own router, guarded by `auth::require_preview_token` rather than
    // `auth::require_bearer_token` — for the same reason `preview_routes`
    // below is: neither a browser's nor Node's native `WebSocket`
    // constructor can set a custom `Authorization` header, so every
    // WebSocket-upgrading endpoint (PTY, and streamed exec-stream logs)
    // needs the same `?token=` query-parameter fallback preview URLs
    // already use.
    let websocket_routes = Router::new()
        .route("/sandboxes/:id/pty", get(routes::pty::pty_session))
        .route("/sandboxes/:id/exec-stream/:session_id/logs", get(routes::logs::attach_logs))
        .merge(routes::tunnel::ws_router())
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_preview_token));

    let drive_routes = Router::new()
        .route("/drives", post(routes::drives::create_drive).get(routes::drives::list_drives))
        .route("/drives/:id", delete(routes::drives::delete_drive))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    let image_routes = Router::new()
        .route("/images", post(routes::images::create_image).get(routes::images::list_images))
        .route("/images/:id", delete(routes::images::delete_image))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    let pool_routes = Router::new()
        .route("/pools", post(routes::pool::create_pool).get(routes::pool::list_pools))
        .route("/pools/:id", delete(routes::pool::delete_pool))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    let snapshot_routes = routes::snapshot::router()
        .merge(routes::snapshot_history::router())
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    // Its own router, guarded by `auth::require_preview_token` rather than
    // `auth::require_bearer_token` — see that middleware's doc comment for
    // why a preview URL needs different auth handling than the rest of the
    // `/sandboxes*` API.
    let preview_routes = Router::new()
        .route("/sandboxes/:id/preview/:port", any(routes::preview::preview_root))
        // `*path` only matches when there's at least one character after
        // the trailing `/` — a bare trailing slash with nothing after it
        // (exactly what `previewUrl()`'s default `path: "/"` produces in
        // every SDK) matches neither this route nor the bare one above
        // without this third explicit registration. Found live: every
        // preview request with the SDKs' own default path 404'd.
        .route("/sandboxes/:id/preview/:port/", any(routes::preview::preview_root))
        .route("/sandboxes/:id/preview/:port/*path", any(routes::preview::preview_path))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_preview_token));

    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(routes::metrics::metrics))
        .merge(sandbox_routes)
        .merge(drive_routes)
        .merge(image_routes)
        .merge(pool_routes)
        .merge(snapshot_routes)
        .merge(preview_routes)
        .merge(websocket_routes)
        .layer(TraceLayer::new_for_http())
        // Outermost: wraps everything above, including `TraceLayer`, so a
        // caller-supplied or freshly generated request id is already
        // established as the active span before any per-request logging
        // happens, and `TraceLayer`'s own request-complete log line ends
        // up correlated too. See `request_id`'s module doc for how that
        // correlation then survives into `sandkiln-vmm` calls made from
        // inside `spawn_blocking` (`tracing_util::spawn_blocking_in_current_span`).
        .layer(middleware::from_fn(request_id::correlate))
        .with_state(state)
}

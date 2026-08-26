mod config;
mod error;
mod routes;
mod sandbox;
mod state;

use axum::routing::{delete, get, post};
use axum::Router;
use config::Config;
use state::AppState;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env();
    let listen_addr = config.listen_addr.clone();
    let state = Arc::new(AppState::new(config));

    let app = Router::new()
        .route("/sandboxes", post(routes::create_sandbox).get(routes::list_sandboxes))
        .route("/sandboxes/:id", delete(routes::stop_sandbox))
        .route("/sandboxes/:id/exec", post(routes::exec))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state);

    tracing::info!(%listen_addr, "sandkiln daemon starting");
    let listener = tokio::net::TcpListener::bind(&listen_addr).await.expect("bind listen address");
    axum::serve(listener, app).await.expect("server error");
}

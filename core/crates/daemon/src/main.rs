mod config;
mod error;
mod routes;
mod sandbox;
mod state;

use axum::routing::{delete, get, post};
use axum::Router;
use config::Config;
use sandkiln_vmm::network::{self, NetworkManager};
use state::AppState;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env();
    let listen_addr = config.listen_addr.clone();

    let uplink = match config.uplink_iface.clone() {
        Some(iface) => iface,
        None => network::detect_default_iface().expect("detect uplink interface (or set SANDKILN_UPLINK_IFACE)"),
    };
    let net_manager = NetworkManager::new(config.bridge_name.clone(), config.bridge_gateway, uplink.clone());
    net_manager.ensure_ready().expect("set up sandbox network bridge (needs CAP_NET_ADMIN — see scripts/grant-net-admin.sh)");
    tracing::info!(bridge = %config.bridge_name, gateway = %config.bridge_gateway, %uplink, "sandbox network ready");

    let state = Arc::new(AppState::new(config, net_manager));

    let app = Router::new()
        .route("/sandboxes", post(routes::create_sandbox).get(routes::list_sandboxes))
        .route("/sandboxes/:id", delete(routes::stop_sandbox))
        .route("/sandboxes/:id/exec", post(routes::exec))
        .route("/healthz", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    tracing::info!(%listen_addr, "sandkiln daemon starting");
    let listener = tokio::net::TcpListener::bind(&listen_addr).await.expect("bind listen address");
    axum::serve(listener, app).await.expect("server error");
}

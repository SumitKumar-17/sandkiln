mod auth;
mod config;
mod error;
mod routes;
mod sandbox;
mod state;

use axum::middleware;
use axum::routing::{delete, get, post};
use axum::Router;
use config::Config;
use sandkiln_vmm::network::{self, NetworkManager};
use state::AppState;
use std::sync::Arc;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

/// Not `#[tokio::main]`: the ambient-capability raise below has to happen
/// before the Tokio runtime spawns its worker/blocking threads, or those
/// threads clone credentials from before the raise and never see it —
/// `#[tokio::main]` starts the runtime as part of macro-generated code
/// that runs before this function body does.
fn main() {
    raise_net_admin_ambient();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
        .block_on(async_main());
}

async fn async_main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = Config::from_env();
    let listen_addr = config.listen_addr.clone();

    let uplink = match config.uplink_iface.clone() {
        Some(iface) => iface,
        None => network::detect_default_iface().expect("detect uplink interface (or set SANDKILN_UPLINK_IFACE)"),
    };
    let tap_pool = (0..config.tap_pool_size).map(|i| format!("{}{i}", config.tap_pool_prefix));
    let net_manager = NetworkManager::new(config.bridge_name.clone(), config.bridge_gateway, uplink.clone(), tap_pool);
    net_manager.ensure_ready().expect("set up sandbox network bridge (needs CAP_NET_ADMIN — see scripts/grant-net-admin.sh)");
    tracing::info!(bridge = %config.bridge_name, gateway = %config.bridge_gateway, %uplink, "sandbox network ready");

    let auth_enabled = config.auth_token.is_some();
    let state = Arc::new(AppState::new(config, net_manager));

    let sandbox_routes = Router::new()
        .route("/sandboxes", post(routes::create_sandbox).get(routes::list_sandboxes))
        .route("/sandboxes/:id", delete(routes::stop_sandbox))
        .route("/sandboxes/:id/exec", post(routes::exec))
        .route("/sandboxes/:id/read-file", post(routes::read_file))
        .route("/sandboxes/:id/write-file", post(routes::write_file))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(sandbox_routes)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    if !auth_enabled {
        tracing::warn!("SANDKILN_AUTH_TOKEN not set — the daemon's API is unauthenticated");
    }
    tracing::info!(%listen_addr, "sandkiln daemon starting");
    let listener = tokio::net::TcpListener::bind(&listen_addr).await.expect("bind listen address");
    axum::serve(listener, app).await.expect("server error");
}

/// The daemon needs CAP_NET_ADMIN itself (via `setcap ...+eip`, see
/// scripts/grant-net-admin.sh) to manage tap devices and iptables rules —
/// but that alone doesn't reach the `ip`/`iptables` child processes it
/// shells out to. Raising the capability into the ambient set makes those
/// children inherit it too.
fn raise_net_admin_ambient() {
    use caps::{CapSet, Capability};
    // Ambient-raising a capability requires it in both Permitted and
    // Inheritable first. The file capability sets Permitted at exec time,
    // but Inheritable is inherited from the parent shell (normally
    // empty) rather than set from the file — so it has to be added here,
    // which is allowed precisely because it's already in Permitted.
    caps::raise(None, CapSet::Inheritable, Capability::CAP_NET_ADMIN)
        .expect("add CAP_NET_ADMIN to the inheritable set — grant the file capability first with scripts/grant-net-admin.sh");
    caps::raise(None, CapSet::Ambient, Capability::CAP_NET_ADMIN)
        .expect("raise CAP_NET_ADMIN into the ambient set");
}

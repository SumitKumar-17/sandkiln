mod auth;
mod config;
mod error;
mod idle_reaper;
mod metrics;
mod request_id;
mod routes_drives;
mod routes_exec;
mod routes_metrics;
mod routes_preview;
mod routes_sandbox;
mod routes_sandbox_name;
mod routes_snapshot;
mod sandbox;
mod snapshot;
mod state;
mod tracing_util;

use axum::middleware;
use axum::routing::{any, delete, get, post};
use axum::Router;
use config::{Config, LogFormat};
use sandkiln_vmm::drive::DriveStore;
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
    let config = Config::from_env();
    init_tracing(config.log_format);
    let listen_addr = config.listen_addr.clone();

    let uplink = match config.uplink_iface.clone() {
        Some(iface) => iface,
        None => network::detect_default_iface().expect("detect uplink interface (or set SANDKILN_UPLINK_IFACE)"),
    };
    let tap_pool = (0..config.tap_pool_size).map(|i| format!("{}{i}", config.tap_pool_prefix));
    let net_manager = NetworkManager::new(config.bridge_name.clone(), config.bridge_gateway, uplink.clone(), tap_pool);
    net_manager.ensure_ready().expect("set up sandbox network bridge (needs CAP_NET_ADMIN — see scripts/grant-net-admin.sh)");
    tracing::info!(bridge = %config.bridge_name, gateway = %config.bridge_gateway, %uplink, "sandbox network ready");

    let drives = DriveStore::new(&config.drives_dir)
        .expect("create/verify the persistent drives directory (SANDKILN_DRIVES_DIR)");
    tracing::info!(drives_dir = %config.drives_dir.display(), "persistent drive storage ready");

    // Must happen before the HTTP listener starts accepting connections
    // and before `AppState` takes ownership of `net_manager`: a
    // reconciled snapshot's tap device has to be pulled out of
    // `net_manager`'s free pool (via `NetworkManager::reserve`, called
    // from `reconcile`) before any live `POST /sandboxes` request can
    // race it for the same tap.
    let reconciled_snapshots = snapshot::reconcile(&net_manager);
    tracing::info!(count = reconciled_snapshots.len(), "reconciled snapshots from disk");

    match &config.jailer {
        Some(jailer_cfg) => {
            std::fs::create_dir_all(&jailer_cfg.chroot_base_dir)
                .expect("create/verify the jailer chroot base dir (SANDKILN_JAILER_CHROOT_BASE_DIR)");
            assert!(
                jailer_cfg.jailer_bin.is_file(),
                "SANDKILN_JAILER_BIN does not point at a file: {} — run scripts/install-firecracker.sh first",
                jailer_cfg.jailer_bin.display()
            );
            tracing::info!(
                jailer_bin = %jailer_cfg.jailer_bin.display(),
                chroot_base_dir = %jailer_cfg.chroot_base_dir.display(),
                uid_gid_range_start = *jailer_cfg.uid_gid_range.start(),
                uid_gid_range_end = *jailer_cfg.uid_gid_range.end(),
                "jailer-based sandbox boot enabled — sandboxes will boot chroot'd, cgroup-limited, under a dedicated uid/gid"
            );
        }
        None => {
            tracing::info!(
                "jailer-based sandbox boot disabled (SANDKILN_JAILER_ENABLED not set) — \
                 sandboxes boot via a direct Firecracker process spawn, see ROADMAP.md's Security hardening section"
            );
        }
    }

    let auth_enabled = config.auth_token.is_some();
    let idle_timeout = config.idle_timeout;
    let state = Arc::new(AppState::new(config, net_manager, drives, reconciled_snapshots));

    if let Some(idle_timeout) = idle_timeout {
        tracing::info!(idle_timeout_secs = idle_timeout.as_secs(), "idle sandbox reaper enabled");
        tokio::spawn(idle_reaper::run(state.clone(), idle_timeout));
    }

    let sandbox_routes = Router::new()
        .route("/sandboxes", post(routes_sandbox::create_sandbox).get(routes_sandbox::list_sandboxes))
        .route("/sandboxes/get-or-create", post(routes_sandbox_name::get_or_create_sandbox))
        .route("/sandboxes/by-name/:name", get(routes_sandbox_name::get_sandbox_by_name))
        .route("/sandboxes/:id", delete(routes_sandbox::stop_sandbox))
        .route("/sandboxes/:id/exec", post(routes_exec::exec))
        .route("/sandboxes/:id/read-file", post(routes_exec::read_file))
        .route("/sandboxes/:id/write-file", post(routes_exec::write_file))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    let drive_routes = Router::new()
        .route("/drives", post(routes_drives::create_drive).get(routes_drives::list_drives))
        .route("/drives/:id", delete(routes_drives::delete_drive))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    let snapshot_routes =
        routes_snapshot::router().route_layer(middleware::from_fn_with_state(state.clone(), auth::require_bearer_token));

    // Its own router, guarded by `auth::require_preview_token` rather than
    // `auth::require_bearer_token` — see that middleware's doc comment for
    // why a preview URL needs different auth handling than the rest of the
    // `/sandboxes*` API.
    let preview_routes = Router::new()
        .route("/sandboxes/:id/preview/:port", any(routes_preview::preview_root))
        // `*path` only matches when there's at least one character after
        // the trailing `/` — a bare trailing slash with nothing after it
        // (exactly what `previewUrl()`'s default `path: "/"` produces in
        // every SDK) matches neither this route nor the bare one above
        // without this third explicit registration. Found live: every
        // preview request with the SDKs' own default path 404'd.
        .route("/sandboxes/:id/preview/:port/", any(routes_preview::preview_root))
        .route("/sandboxes/:id/preview/:port/*path", any(routes_preview::preview_path))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_preview_token));

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/metrics", get(routes_metrics::metrics))
        .merge(sandbox_routes)
        .merge(drive_routes)
        .merge(snapshot_routes)
        .merge(preview_routes)
        .layer(TraceLayer::new_for_http())
        // Outermost: wraps everything above, including `TraceLayer`, so a
        // caller-supplied or freshly generated request id is already
        // established as the active span before any per-request logging
        // happens, and `TraceLayer`'s own request-complete log line ends
        // up correlated too. See `request_id`'s module doc for how that
        // correlation then survives into `sandkiln-vmm` calls made from
        // inside `spawn_blocking` (`tracing_util::spawn_blocking_in_current_span`).
        .layer(middleware::from_fn(request_id::correlate))
        .with_state(state);

    if !auth_enabled {
        tracing::warn!("SANDKILN_AUTH_TOKEN not set — the daemon's API is unauthenticated");
    }
    tracing::info!(%listen_addr, "sandkiln daemon starting");
    let listener = tokio::net::TcpListener::bind(&listen_addr).await.expect("bind listen address");
    axum::serve(listener, app).await.expect("server error");
}

/// `.json()` and the default pretty layer are different builder types, so
/// this can't be one `tracing_subscriber::fmt()` chain with a branch in
/// the middle — each arm builds and `init()`s its own subscriber.
fn init_tracing(log_format: LogFormat) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    match log_format {
        LogFormat::Json => tracing_subscriber::fmt().with_env_filter(env_filter).json().init(),
        LogFormat::Pretty => tracing_subscriber::fmt().with_env_filter(env_filter).init(),
    }
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

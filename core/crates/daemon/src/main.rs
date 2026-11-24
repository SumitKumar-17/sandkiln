mod config;
mod http;
mod lifecycle;
mod log_session;
mod metrics;
mod rlimit;
mod sandbox;
mod snapshots;
mod state;
mod storage;
mod tracing_util;

use config::{Config, LogFormat};
use sandkiln_store::HistoryStore;
use sandkiln_vmm::drive::DriveStore;
use sandkiln_vmm::image::ImageStore;
use sandkiln_vmm::network::{self, NetworkManager};
use state::AppState;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Not `#[tokio::main]`: the ambient-capability raise below has to happen
/// before the Tokio runtime spawns its worker/blocking threads, or those
/// threads clone credentials from before the raise and never see it —
/// `#[tokio::main]` starts the runtime as part of macro-generated code
/// that runs before this function body does.
fn main() {
    use config::cli::{self, Command};
    let cli = match cli::parse(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("sandkilnd: {e}");
            std::process::exit(2);
        }
    };
    match cli.command {
        Command::Help => return print!("{}", cli::USAGE),
        Command::Version => return println!("sandkilnd {}", env!("CARGO_PKG_VERSION")),
        Command::ConfigReference => return print!("{}", config::reference_markdown()),
        Command::Run | Command::CheckConfig | Command::PrintConfig => {}
    }
    let loaded = match Config::load(cli.config_file.as_deref()) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprint!("sandkilnd: {e}");
            std::process::exit(2);
        }
    };
    match cli.command {
        Command::PrintConfig => return print!("{}", loaded.config.render_effective(loaded.file.as_deref())),
        Command::CheckConfig => {
            for w in &loaded.warnings {
                eprintln!("warning: {w}");
            }
            return println!("configuration OK ({})", loaded.file.as_deref().map_or("environment and defaults".into(), |p| p.display().to_string()));
        }
        _ => {}
    }

    raise_net_admin_ambient();
    if let Err(e) = rlimit::raise_nofile_to_hard() {
        eprintln!("warning: could not raise the open-file limit: {e}");
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
        .block_on(async_main(loaded));
}

async fn async_main(loaded: config::Loaded) {
    let config = loaded.config;
    init_tracing(config.log_format);
    match &loaded.file {
        Some(path) => tracing::info!(config_file = %path.display(), "loaded configuration file"),
        None => tracing::info!("no configuration file; using environment and defaults"),
    }
    for warning in &loaded.warnings {
        tracing::warn!("{warning}");
    }
    let listen_addr = config.listen_addr.clone();

    storage::init(storage::StorageLayout::new(&config.state_dir, &config.rootfs_dir))
        .expect("create the state/rootfs directories (SANDKILN_STATE_DIR, SANDKILN_ROOTFS_DIR)");
    tracing::info!(state_dir = %config.state_dir.display(), rootfs_dir = %config.rootfs_dir.display(), "daemon storage ready");

    let uplink = match config.uplink_iface.clone() {
        Some(iface) => iface,
        None => network::detect_default_iface().expect("detect uplink interface (or set SANDKILN_UPLINK_IFACE)"),
    };
    let tap_pool = (0..config.tap_pool_size).map(|i| format!("{}{i}", config.tap_pool_prefix));
    let net_manager = NetworkManager::new(config.bridge_name.clone(), config.bridge_gateway, uplink.clone(), tap_pool);
    net_manager.ensure_ready().expect("set up sandbox network bridge (needs CAP_NET_ADMIN — see scripts/host-setup/grant-net-admin.sh)");
    tracing::info!(bridge = %config.bridge_name, gateway = %config.bridge_gateway, %uplink, "sandbox network ready");

    let drives = DriveStore::new(&config.drives_dir)
        .expect("create/verify the persistent drives directory (SANDKILN_DRIVES_DIR)");
    tracing::info!(drives_dir = %config.drives_dir.display(), "persistent drive storage ready");

    let images = ImageStore::new(&config.images_dir)
        .expect("create/verify the registered images directory (SANDKILN_IMAGES_DIR)");
    tracing::info!(images_dir = %config.images_dir.display(), "registered image storage ready");

    // Must happen before the HTTP listener starts accepting connections
    // and before `AppState` takes ownership of `net_manager`: a
    // reconciled snapshot's tap device has to be pulled out of
    // `net_manager`'s free pool (via `NetworkManager::reserve`, called
    // from `reconcile`) before any live `POST /sandboxes` request can
    // race it for the same tap.
    let reconciled_snapshots = snapshots::held::reconcile::reconcile(&net_manager, &config.archive_dir);
    tracing::info!(count = reconciled_snapshots.len(), "reconciled snapshots from disk");

    // Order relative to `reconciled_snapshots` above doesn't matter --
    // unlike that call, this never touches `net_manager` at all (a
    // retired checkpoint holds no live lease to reclaim, see
    // `snapshot_history`'s module doc comment).
    let reconciled_retired = snapshots::retired::reconcile();
    tracing::info!(count = reconciled_retired.len(), "reconciled retired snapshot checkpoints from disk");

    let history = HistoryStore::open(&config.history_db_path)
        .expect("open/create the sandbox history database (SANDKILN_HISTORY_DB_PATH)");
    // Same reasoning as reconciling snapshots above, before the listener
    // binds: any record still marked "live" at this point belongs to a
    // daemon process that is no longer running, and its Firecracker
    // process (if it even still exists) is unconditionally orphaned —
    // see sandkiln-store's own module doc comment for why this isn't a
    // heuristic.
    let orphaned_count = history
        .mark_unended_as_orphaned_on_startup(std::time::SystemTime::now())
        .expect("mark orphaned sandbox history records at startup");
    tracing::info!(count = orphaned_count, "marked orphaned sandbox history records from a prior daemon run");

    match &config.jailer {
        Some(jailer_cfg) => {
            std::fs::create_dir_all(&jailer_cfg.chroot_base_dir)
                .expect("create/verify the jailer chroot base dir (SANDKILN_JAILER_CHROOT_BASE_DIR)");
            assert!(
                jailer_cfg.jailer_bin.is_file(),
                "SANDKILN_JAILER_BIN does not point at a file: {} — run scripts/host-setup/install-firecracker.sh first",
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
                 sandboxes boot via a direct Firecracker process spawn, see `docs/design-notes/security-hardening.md`"
            );
        }
    }

    let auth_enabled = config.auth_token.is_some();
    let idle_timeout = config.idle_timeout;
    let auto_suspend_timeout = config.auto_suspend_timeout;
    let archive_timeout = config.archive_timeout;
    let archive_dir = config.archive_dir.clone();
    let state = Arc::new(AppState::new(config, net_manager, drives, images, reconciled_snapshots, reconciled_retired, history));

    if let Some(auto_suspend_timeout) = auto_suspend_timeout {
        tracing::info!(auto_suspend_timeout_secs = auto_suspend_timeout.as_secs(), "idle sandbox auto-suspend enabled");
    }
    if let Some(idle_timeout) = idle_timeout {
        tracing::info!(idle_timeout_secs = idle_timeout.as_secs(), "idle sandbox destroy reaper enabled");
    }
    if let Some(archive_timeout) = archive_timeout {
        tracing::info!(archive_timeout_secs = archive_timeout.as_secs(), archive_dir = %archive_dir.display(), "idle snapshot archiving enabled");
    }
    // Unconditional -- see `idle_reaper`'s own module doc comment (and
    // `pool_replenisher`'s identical reasoning below) for why a tick with
    // nothing configured at all is a cheap no-op scan, not worth gating
    // behind whether any of the three timeouts are actually set.
    tokio::spawn(lifecycle::idle_reaper::run(state.clone(), idle_timeout, auto_suspend_timeout, archive_timeout, archive_dir));
    tokio::spawn(lifecycle::pool_replenisher::run(state.clone()));

    let app = http::build_router(state.clone());

    if !auth_enabled {
        tracing::warn!("SANDKILN_AUTH_TOKEN not set — the daemon's API is unauthenticated");
    }
    tracing::info!(%listen_addr, "sandkiln daemon starting");
    let listener = tokio::net::TcpListener::bind(&listen_addr).await.expect("bind listen address");
    // Stop accepting on a signal, give in-flight requests a grace period, then
    // snapshot every live sandbox so a restart does not strand them.
    let stop_accepting = Arc::new(tokio::sync::Notify::new());
    let server = axum::serve(listener, app).with_graceful_shutdown({
        let stop_accepting = stop_accepting.clone();
        async move { stop_accepting.notified().await }
    });
    tokio::select! {
        result = server => result.expect("server error"),
        _ = async {
            lifecycle::shutdown::signal().await;
            stop_accepting.notify_one();
            tokio::time::sleep(lifecycle::shutdown::REQUEST_GRACE).await;
        } => tracing::warn!("in-flight requests did not finish within the grace period"),
    }
    lifecycle::shutdown::drain_sandboxes(state).await;
    tracing::info!("shutdown complete");
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
/// scripts/host-setup/grant-net-admin.sh) to manage tap devices and iptables rules —
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
        .expect("add CAP_NET_ADMIN to the inheritable set — grant the file capability first with scripts/host-setup/grant-net-admin.sh");
    caps::raise(None, CapSet::Ambient, Capability::CAP_NET_ADMIN)
        .expect("raise CAP_NET_ADMIN into the ambient set");
}

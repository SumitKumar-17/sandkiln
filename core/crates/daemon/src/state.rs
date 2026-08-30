use crate::config::Config;
use crate::metrics::Metrics;
use crate::routes_preview::PreviewClient;
use crate::sandbox::Sandbox;
use crate::snapshot::Snapshot;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use sandkiln_vmm::drive::DriveStore;
use sandkiln_vmm::jailer::JailerIdPool;
use sandkiln_vmm::network::NetworkManager;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

pub struct AppState {
    pub config: Config,
    pub network: NetworkManager,
    pub drives: DriveStore,
    /// `Some` exactly when `config.jailer` is `Some` — the pool of
    /// uid/gid pairs `routes_sandbox::create_sandbox` leases from for
    /// each jailed boot, released back on `stop_sandbox_by_id`. Built
    /// here rather than passed in separately since its range comes
    /// straight out of `config.jailer`.
    pub jailer_ids: Option<JailerIdPool>,
    pub sandboxes: Mutex<HashMap<String, Sandbox>>,
    pub snapshots: Mutex<HashMap<String, Snapshot>>,
    pub metrics: Metrics,
    /// Reused across every `/preview` proxy request rather than built
    /// per-request, so repeated hits on one dev server benefit from
    /// `hyper-util`'s connection pooling instead of a fresh TCP handshake
    /// (and, for a WebSocket-using dev server later, from a client
    /// already wired for keep-alive) every time.
    pub preview_client: PreviewClient,
}

impl AppState {
    /// `snapshots` is the result of `crate::snapshot::reconcile` run
    /// against the on-disk snapshot store before this is called — passed
    /// in rather than always starting empty so a daemon restart doesn't
    /// silently orphan every snapshot that was durable on disk (see
    /// `main.rs`).
    pub fn new(config: Config, network: NetworkManager, drives: DriveStore, snapshots: HashMap<String, Snapshot>) -> Self {
        let jailer_ids = config.jailer.as_ref().map(|j| JailerIdPool::new(j.uid_gid_range.clone()));
        Self {
            config,
            network,
            drives,
            jailer_ids,
            sandboxes: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(snapshots),
            metrics: Metrics::new(),
            preview_client: build_preview_client(),
        }
    }

    /// Where a drive id is currently held, if anywhere — a running
    /// sandbox, or a snapshot with it frozen into saved state (Firecracker
    /// bakes a drive's host path into the snapshot the same way it does
    /// network config, so a snapshotted drive is still "in use" even
    /// though no `Vm` is running). Checked wherever an operation would
    /// conflict with the drive still being held: attaching it to another
    /// sandbox, or deleting it outright.
    pub fn drive_holder(&self, drive_id: &str) -> Option<String> {
        if let Some(sandbox) = self.sandboxes.lock().unwrap().values().find(|s| s.attached_drives.iter().any(|d| d == drive_id))
        {
            return Some(format!("sandbox {}", sandbox.id));
        }
        if let Some(snapshot) =
            self.snapshots.lock().unwrap().values().find(|s| s.attached_drives.iter().any(|d| d == drive_id))
        {
            return Some(format!("snapshot {}", snapshot.id));
        }
        None
    }
}

/// A short connect timeout is what actually turns "guest port isn't
/// listening" into a fast, clear error — a refused connection fails
/// immediately either way, but a black-holed one (SYN silently dropped,
/// e.g. a guest firewall rule) would otherwise hang until the request-level
/// timeout in `Config::preview_timeout`, which is tuned for slow dev-server
/// compiles, not connection setup.
fn build_preview_client() -> PreviewClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(Duration::from_secs(5)));
    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector)
}

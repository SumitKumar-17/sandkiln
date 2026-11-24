//! `AppState`: the daemon's shared state (config, network, stores, and the
//! in-memory maps of live sandboxes, held and retired snapshots, pools and
//! tunnels), plus the drive/mount record types.
//!
//! Ownership queries live in `holders` ("who holds this drive, image or tap")
//! and naming in `naming`; both extend `AppState` and are re-exported here so
//! callers keep using `crate::state::*`.

mod holders;
mod naming;

pub use holders::{can_attach_read_only, describe_drive_holders};
pub use naming::NameResolution;

use crate::config::Config;
use crate::metrics::Metrics;
use crate::lifecycle::pool::Pool;
use crate::http::routes::preview::PreviewClient;
use crate::sandbox::Sandbox;
use crate::snapshots::held::Snapshot;
use crate::snapshots::retired::RetiredSnapshot;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use sandkiln_store::HistoryStore;
use sandkiln_vmm::drive::DriveStore;
use sandkiln_vmm::image::ImageStore;
use sandkiln_vmm::jailer::JailerIdPool;
use sandkiln_vmm::network::NetworkManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;


/// A drive attachment plus its read-only flag — carried as a pair (not a
/// bare id) because whether a *new* attach may coexist with existing ones
/// depends on both. See `can_attach_read_only`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedDrive {
    pub drive_id: String,
    pub read_only: bool,
}

/// One remote object-store mount active inside a sandbox — see
/// `crate::http::routes::mounts`. No credentials here: they're written straight
/// into the guest (a passwd file, never a command-line arg `ps aux`
/// could see) and never touch daemon state, so nothing secret is at risk
/// from persisting or logging this struct.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    pub id: String,
    pub bucket: String,
    /// Required, not defaulted to a provider — a mount's target is
    /// always explicit.
    pub endpoint: String,
    pub mount_path: String,
    pub read_only: bool,
}

pub struct AppState {
    pub config: Config,
    pub network: NetworkManager,
    pub drives: DriveStore,
    pub images: ImageStore,
    /// `Some` iff `config.jailer` is `Some` — uid/gid pairs leased per
    /// jailed boot (`lifecycle::create::create_sandbox_cold`), released on stop.
    pub jailer_ids: Option<JailerIdPool>,
    pub sandboxes: Mutex<HashMap<String, Sandbox>>,
    pub snapshots: Mutex<HashMap<String, Snapshot>>,
    /// Retired checkpoints ("time-travel restore", see
    /// `crate::snapshots::retired`). A separate map, not a flag on
    /// `Snapshot`: a retired checkpoint holds no live `Lease` at all,
    /// structurally unlike every `snapshots` entry.
    pub retired_snapshots: Mutex<HashMap<String, RetiredSnapshot>>,
    /// Lazily-created per-name mutex, serializing concurrent claims/
    /// resolves of the *same* name (`create_sandbox`,
    /// `get_or_create_sandbox`) without blocking unrelated names. See
    /// `AppState::lock_name`.
    pub name_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Refcounted claims on an image id mid-boot, before its `Sandbox` is
    /// visible in `sandboxes` — closes the gap where `DELETE /images/:id`
    /// could delete a file an in-progress clone is still reading. Keyed
    /// by image id, refcounted, since multiple boots can share one image.
    pending_image_boots: Mutex<HashMap<String, u32>>,
    pub metrics: Metrics,
    /// Daemon start, for `/status` uptime.
    pub started_at: std::time::Instant,
    /// Reused across every `/preview` request (not built per-request) so
    /// repeat hits on one dev server reuse `hyper-util`'s connection pool.
    pub preview_client: PreviewClient,
    /// Durable sandbox history (`sandkiln-store`) — separate concern from
    /// `sandboxes`/`snapshots`; see that crate's own doc comment.
    pub history: HistoryStore,
    /// Configured pre-warmed pools, keyed by caller-given id. In-memory
    /// only, unlike `snapshots` — see `crate::lifecycle::pool`.
    pub pools: Mutex<HashMap<String, Pool>>,
    /// Tap devices with a restore in flight. Closes a race
    /// `tap_device_holder` alone can't: two different retired checkpoints
    /// in one lineage share a frozen tap/IP/MAC (`crate::snapshots::retired`),
    /// so two concurrent restores could both pass a plain liveness check
    /// and then race `NetworkManager::reserve`, which doesn't itself
    /// detect double-reservation (it's built for the startup-reconcile
    /// case, where nothing else can be live yet). A plain set, not a
    /// refcount like `pending_image_boots` — only one restore per tap
    /// device can ever be in flight.
    pending_tap_restores: Mutex<std::collections::HashSet<String>>,
    /// One entry per tunnel between `POST /sandboxes/:id/tunnel`
    /// (register) and the matching `GET .../tunnel/:tunnel_id/ws`
    /// (take) — see `http/routes/tunnel.rs`. The receiving half of
    /// `sandkiln_vmm::tunnel::register`'s channel; removed the moment a
    /// WebSocket takes it, so a second WS attempt on the same tunnel_id
    /// gets a clear `409` instead of silently racing the first for
    /// connections.
    pub tunnels: Mutex<HashMap<String, std::sync::mpsc::Receiver<sandkiln_vmm::tunnel::TunnelConnection>>>,
}


impl AppState {
    /// `snapshots` comes from `crate::snapshots::held::reconcile::reconcile` against disk,
    /// passed in rather than started empty so a restart doesn't orphan
    /// durable snapshots (see `main.rs`).
    pub fn new(
        config: Config,
        network: NetworkManager,
        drives: DriveStore,
        images: ImageStore,
        snapshots: HashMap<String, Snapshot>,
        retired_snapshots: HashMap<String, RetiredSnapshot>,
        history: HistoryStore,
    ) -> Self {
        let jailer_ids = config.jailer.as_ref().map(|j| JailerIdPool::new(j.uid_gid_range.clone()));
        Self {
            config,
            network,
            drives,
            images,
            jailer_ids,
            sandboxes: Mutex::new(HashMap::new()),
            snapshots: Mutex::new(snapshots),
            retired_snapshots: Mutex::new(retired_snapshots),
            name_locks: Mutex::new(HashMap::new()),
            pending_image_boots: Mutex::new(HashMap::new()),
            metrics: Metrics::new(),
            started_at: std::time::Instant::now(),
            preview_client: build_preview_client(),
            history,
            pools: Mutex::new(HashMap::new()),
            pending_tap_restores: Mutex::new(std::collections::HashSet::new()),
            tunnels: Mutex::new(HashMap::new()),
        }
    }

    /// Copies a sandbox's guest-agent endpoint out of the map so the caller can
    /// release the daemon-wide `sandboxes` lock *before* making a call that can
    /// block (a dead guest costs up to ~5s of retries). Never call
    /// `sandbox.vm.call(..)` while holding the lock.
    pub fn agent_endpoint(&self, id: &str) -> Option<sandkiln_vmm::vm::AgentEndpoint> {
        self.sandboxes.lock().unwrap().get(id).map(|s| s.vm.agent_endpoint())
    }
}

/// A short connect timeout turns a black-holed SYN (e.g. a guest firewall
/// rule) into a fast error instead of hanging until `Config::preview_timeout`,
/// which is tuned for slow dev-server compiles, not connection setup.
fn build_preview_client() -> PreviewClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(Duration::from_secs(5)));
    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector)
}

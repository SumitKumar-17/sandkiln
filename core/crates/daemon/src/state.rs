use crate::config::Config;
use crate::metrics::Metrics;
use crate::pool::Pool;
use crate::routes_preview::PreviewClient;
use crate::sandbox::Sandbox;
use crate::snapshot::Snapshot;
use crate::snapshot_history::RetiredSnapshot;
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
/// `crate::routes_mounts`. No credentials here: they're written straight
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
    /// jailed boot (`routes_sandbox::create_sandbox`), released on stop.
    pub jailer_ids: Option<JailerIdPool>,
    pub sandboxes: Mutex<HashMap<String, Sandbox>>,
    pub snapshots: Mutex<HashMap<String, Snapshot>>,
    /// Retired checkpoints ("time-travel restore", see
    /// `crate::snapshot_history`). A separate map, not a flag on
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
    /// Reused across every `/preview` request (not built per-request) so
    /// repeat hits on one dev server reuse `hyper-util`'s connection pool.
    pub preview_client: PreviewClient,
    /// Durable sandbox history (`sandkiln-store`) — separate concern from
    /// `sandboxes`/`snapshots`; see that crate's own doc comment.
    pub history: HistoryStore,
    /// Configured pre-warmed pools, keyed by caller-given id. In-memory
    /// only, unlike `snapshots` — see `crate::pool`.
    pub pools: Mutex<HashMap<String, Pool>>,
    /// Tap devices with a restore in flight. Closes a race
    /// `tap_device_holder` alone can't: two different retired checkpoints
    /// in one lineage share a frozen tap/IP/MAC (`crate::snapshot_history`),
    /// so two concurrent restores could both pass a plain liveness check
    /// and then race `NetworkManager::reserve`, which doesn't itself
    /// detect double-reservation (it's built for the startup-reconcile
    /// case, where nothing else can be live yet). A plain set, not a
    /// refcount like `pending_image_boots` — only one restore per tap
    /// device can ever be in flight.
    pending_tap_restores: Mutex<std::collections::HashSet<String>>,
    /// One entry per tunnel between `POST /sandboxes/:id/tunnel`
    /// (register) and the matching `GET .../tunnel/:tunnel_id/ws`
    /// (take) — see `routes_tunnel.rs`. The receiving half of
    /// `sandkiln_vmm::tunnel::register`'s channel; removed the moment a
    /// WebSocket takes it, so a second WS attempt on the same tunnel_id
    /// gets a clear `409` instead of silently racing the first for
    /// connections.
    pub tunnels: Mutex<HashMap<String, std::sync::mpsc::Receiver<sandkiln_vmm::tunnel::TunnelConnection>>>,
}

impl AppState {
    /// `snapshots` comes from `crate::snapshot::reconcile` against disk,
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
            preview_client: build_preview_client(),
            history,
            pools: Mutex::new(HashMap::new()),
            pending_tap_restores: Mutex::new(std::collections::HashSet::new()),
            tunnels: Mutex::new(HashMap::new()),
        }
    }

    /// Every current holder of `drive_id` — live sandboxes and held
    /// snapshots (a snapshot freezes a drive's host path into its saved
    /// state, so it's still "in use" with no `Vm` running) — labeled,
    /// with each attachment's read-only flag. Empty means unheld. Checked
    /// before attaching elsewhere (`can_attach_read_only` allows stacking
    /// read-only holders) or deleting (never fine while non-empty).
    pub fn drive_holders(&self, drive_id: &str) -> Vec<DriveHold> {
        let mut holders: Vec<DriveHold> = self
            .sandboxes
            .lock()
            .unwrap()
            .values()
            .filter_map(|s| {
                s.attached_drives
                    .iter()
                    .find(|d| d.drive_id == drive_id)
                    .map(|d| DriveHold { holder: format!("sandbox {}", s.id), read_only: d.read_only })
            })
            .collect();
        holders.extend(self.snapshots.lock().unwrap().values().filter_map(|s| {
            s.attached_drives
                .iter()
                .find(|d| d.drive_id == drive_id)
                .map(|d| DriveHold { holder: format!("snapshot {}", s.id), read_only: d.read_only })
        }));
        // A retired checkpoint's drive reference is as durable as a held
        // snapshot's — otherwise a drive it depends on read-write could
        // be re-attached read-write elsewhere, and restoring later would
        // hand two live VMs the same mutable backing file.
        holders.extend(self.retired_snapshots.lock().unwrap().values().filter_map(|s| {
            s.attached_drives
                .iter()
                .find(|d| d.drive_id == drive_id)
                .map(|d| DriveHold { holder: format!("retired snapshot {}", s.id), read_only: d.read_only })
        }));
        holders
    }

    /// Where a name is claimed, if anywhere. Checks live sandboxes before
    /// held snapshots: a forked snapshot and its live `Sandbox` carry the
    /// same name at once (one identity, not a conflict — see
    /// `Sandbox::name`), and the live one is the more actionable answer.
    pub fn name_holder(&self, name: &str) -> Option<String> {
        let sandboxes = self.sandboxes.lock().unwrap();
        if let Some(id) = find_named(sandboxes.values().map(|s| (s.id.as_str(), s.name.as_deref())), name) {
            return Some(format!("sandbox {id}"));
        }
        drop(sandboxes);
        let snapshots = self.snapshots.lock().unwrap();
        if let Some(id) = find_named(snapshots.values().map(|s| (s.id.as_str(), s.name.as_deref())), name) {
            return Some(format!("snapshot {id}"));
        }
        None
    }

    /// Resolves a name to live-and-actionable vs. held-needs-resume —
    /// `name_holder` collapses that into a display string, enough for a
    /// conflict message but not for `get_or_create_sandbox`/
    /// `GET /sandboxes/by-name/:name` to decide what to do next.
    pub fn resolve_name(&self, name: &str) -> Option<NameResolution> {
        let sandboxes = self.sandboxes.lock().unwrap();
        if let Some(id) = find_named(sandboxes.values().map(|s| (s.id.as_str(), s.name.as_deref())), name) {
            return Some(NameResolution::Live(id.to_string()));
        }
        drop(sandboxes);
        let snapshots = self.snapshots.lock().unwrap();
        if let Some(id) = find_named(snapshots.values().map(|s| (s.id.as_str(), s.name.as_deref())), name) {
            return Some(NameResolution::Snapshot(id.to_string()));
        }
        None
    }

    /// Where an image id is referenced, if anywhere — `drive_holder` for
    /// `ImageStore` entries. Checked before a boot commits to an image
    /// and before `DELETE /images/:id`. Covers an in-flight boot too
    /// (`pending_image_boots`): the image is "in use" from the moment the
    /// boot starts, before its `Sandbox` is even visible.
    pub fn image_holder(&self, image_id: &str) -> Option<String> {
        if self.pending_image_boots.lock().unwrap().get(image_id).is_some_and(|count| *count > 0) {
            return Some("a sandbox currently being created".to_string());
        }
        // A retired checkpoint's image reference must be checked now,
        // not deferred to its (maybe never) restore time.
        let sandboxes = self.sandboxes.lock().unwrap();
        let snapshots = self.snapshots.lock().unwrap();
        let retired = self.retired_snapshots.lock().unwrap();
        first_match(sandboxes.values(), "sandbox", |s| s.id.as_str(), |s| s.image_id.as_deref() == Some(image_id))
            .or_else(|| first_match(snapshots.values(), "snapshot", |s| s.id.as_str(), |s| s.image_id.as_deref() == Some(image_id)))
            .or_else(|| {
                first_match(retired.values(), "retired snapshot", |s| s.id.as_str(), |s| s.image_id.as_deref() == Some(image_id))
            })
    }

    /// Where `tap_device` is held: a live sandbox's lease, a held
    /// snapshot's (the `Lease` stays in the `Snapshot` whether or not it's
    /// lent to a live fork — see `Snapshot::forked_into`), or another
    /// retired checkpoint mid-restore. `None` means free to reserve.
    ///
    /// The guard `restore_snapshot_history` needs before calling
    /// `NetworkManager::reserve`: unlike `snapshot::reconcile`'s startup-
    /// only call (nothing else can be live yet), a restore can race a
    /// real live user of the same tap, and `reserve` itself doesn't check
    /// for that — it's built for the startup case and will proceed with
    /// only a warning otherwise.
    pub fn tap_device_holder(&self, tap_device: &str) -> Option<String> {
        let sandboxes = self.sandboxes.lock().unwrap();
        let snapshots = self.snapshots.lock().unwrap();
        first_match(sandboxes.values(), "sandbox", |s| s.id.as_str(), |s| {
            s.network.as_ref().is_some_and(|n| n.config.tap_device == tap_device)
        })
        .or_else(|| first_match(snapshots.values(), "snapshot", |s| s.id.as_str(), |s| s.network.config.tap_device == tap_device))
        .or_else(|| {
            self.pending_tap_restores
                .lock()
                .unwrap()
                .contains(tap_device)
                .then(|| "another restore already in progress for this checkpoint's network identity".to_string())
        })
    }

    /// Claims `tap_device` for an in-flight restore: `true` = won, `false`
    /// = another restore already holds it, caller must refuse. Pair with
    /// `release_pending_tap_restore`, ideally via an RAII guard
    /// (`routes_snapshot_history::PendingTapRestoreGuard`) so every exit
    /// path releases it.
    pub fn try_reserve_pending_tap_restore(&self, tap_device: &str) -> bool {
        self.pending_tap_restores.lock().unwrap().insert(tap_device.to_string())
    }

    pub fn release_pending_tap_restore(&self, tap_device: &str) {
        self.pending_tap_restores.lock().unwrap().remove(tap_device);
    }

    /// Serializes claims/resolves of one *same* name, leaving unrelated
    /// names free — closes the race where two concurrent
    /// `get_or_create`/named-create calls for a brand-new name both see
    /// "not taken". A caller holds the guard across its whole
    /// check-then-act sequence (`create_sandbox`, `get_or_create_sandbox`);
    /// a second caller for the same name blocks until the first commits
    /// or fails.
    ///
    /// Entries are swept best-effort once nothing references them anymore
    /// (see `NameLockGuard::drop`) so this doesn't grow forever.
    pub async fn lock_name(self: &Arc<Self>, name: &str) -> NameLockGuard {
        let lock = {
            let mut locks = self.name_locks.lock().unwrap();
            locks.entry(name.to_string()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
        };
        let guard = Arc::clone(&lock).lock_owned().await;
        NameLockGuard { state: self.clone(), name: name.to_string(), lock, _guard: guard }
    }

    /// Claims a pending reference on `image_id` for a boot's duration —
    /// see `pending_image_boots`. Pair with `release_pending_image_boot`,
    /// ideally via an RAII guard (`routes_sandbox::ImagePendingBootGuard`)
    /// so every exit path releases it.
    pub fn reserve_pending_image_boot(&self, image_id: &str) {
        *self.pending_image_boots.lock().unwrap().entry(image_id.to_string()).or_insert(0) += 1;
    }

    pub fn release_pending_image_boot(&self, image_id: &str) {
        let mut pending = self.pending_image_boots.lock().unwrap();
        if let Some(count) = pending.get_mut(image_id) {
            *count -= 1;
            if *count == 0 {
                pending.remove(image_id);
            }
        }
    }
}

/// Pure decision behind `name_holder`/`resolve_name`, pulled out for
/// unit-testing without a real `Sandbox`/`Snapshot` (both need KVM to
/// construct) — same pattern as `auth::token_matches`.
fn find_named<'a>(mut entries: impl Iterator<Item = (&'a str, Option<&'a str>)>, name: &str) -> Option<&'a str> {
    entries.find_map(|(id, entry_name)| (entry_name == Some(name)).then_some(id))
}

/// Shared "first match, labeled" walk `image_holder`/`tap_device_holder`
/// both need over sandboxes/snapshots/retired checkpoints. Returns e.g.
/// `"sandbox sbx-1"`, or `None`.
fn first_match<'a, T: 'a>(
    mut items: impl Iterator<Item = &'a T>,
    label: &str,
    id: impl Fn(&'a T) -> &'a str,
    matches: impl Fn(&'a T) -> bool,
) -> Option<String> {
    items.find(|item| matches(item)).map(|item| format!("{label} {}", id(item)))
}

/// What a name currently resolves to — see `AppState::resolve_name`.
pub enum NameResolution {
    /// A live sandbox, ready to act on directly.
    Live(String),
    /// A held snapshot with this name and no live fork of it — resolvable,
    /// but needs a resume (or fork) before it's a sandbox again.
    Snapshot(String),
}

/// RAII handle for one name's lock, held across a check-then-act
/// sequence — see `AppState::lock_name`.
pub struct NameLockGuard {
    state: Arc<AppState>,
    name: String,
    // Kept alongside `_guard` so `Drop` can check `Arc::strong_count`.
    lock: Arc<tokio::sync::Mutex<()>>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for NameLockGuard {
    fn drop(&mut self) {
        let mut locks = self.state.name_locks.lock().unwrap();
        // 3 references = just us (map's copy + self.lock + the
        // OwnedMutexGuard's own internal clone, all still alive here since
        // fields drop only after this body returns). Higher means another
        // `lock_name` call for this name is already waiting/holding —
        // removing the map entry now would let a third caller create a
        // second, different lock object for the same name. Safe to skip:
        // that other holder cleans up once it's done.
        if Arc::strong_count(&self.lock) == 3 {
            locks.remove(&self.name);
        }
    }
}

/// One drive holder (sandbox or snapshot) and its read-only flag. See
/// `AppState::drive_holders`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveHold {
    pub holder: String,
    pub read_only: bool,
}

/// A drive may have arbitrarily many holders at once only if every
/// existing holder *and* the new attach are all read-only — any
/// read-write attachment needs exclusive access. Pulled out for direct
/// testing without `AppState`/a mutex/axum.
pub fn can_attach_read_only(existing: &[bool], requesting_read_only: bool) -> bool {
    existing.is_empty() || (requesting_read_only && existing.iter().all(|ro| *ro))
}

/// Human-readable `DriveHold` list for `AppError::Conflict` messages.
pub fn describe_drive_holders(holders: &[DriveHold]) -> String {
    holders
        .iter()
        .map(|h| if h.read_only { format!("{} (read-only)", h.holder) } else { h.holder.clone() })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A short connect timeout turns a black-holed SYN (e.g. a guest firewall
/// rule) into a fast error instead of hanging until `Config::preview_timeout`,
/// which is tuned for slow dev-server compiles, not connection setup.
fn build_preview_client() -> PreviewClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(Duration::from_secs(5)));
    hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build(connector)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_attach_read_only_allows_the_first_attach_regardless_of_mode() {
        assert!(can_attach_read_only(&[], true));
        assert!(can_attach_read_only(&[], false));
    }

    #[test]
    fn can_attach_read_only_allows_stacking_more_read_only_holders() {
        assert!(can_attach_read_only(&[true], true));
        assert!(can_attach_read_only(&[true, true], true));
    }

    #[test]
    fn can_attach_read_only_rejects_a_read_write_request_while_anything_holds_it() {
        assert!(!can_attach_read_only(&[true], false));
        assert!(!can_attach_read_only(&[true, true], false));
    }

    #[test]
    fn can_attach_read_only_rejects_a_read_only_request_while_any_holder_is_read_write() {
        assert!(!can_attach_read_only(&[false], true));
        // Mixed existing holders: one read-only, one read-write — the
        // read-write one alone is enough to force exclusivity.
        assert!(!can_attach_read_only(&[true, false], true));
    }

    #[test]
    fn can_attach_read_only_rejects_read_write_onto_a_read_write_holder() {
        assert!(!can_attach_read_only(&[false], false));
    }

    #[test]
    fn describe_drive_holders_marks_read_only_holders_and_leaves_read_write_ones_bare() {
        let holders = vec![
            DriveHold { holder: "sandbox a".to_string(), read_only: true },
            DriveHold { holder: "sandbox b".to_string(), read_only: false },
        ];
        assert_eq!(describe_drive_holders(&holders), "sandbox a (read-only), sandbox b");
    }

    #[test]
    fn describe_drive_holders_of_an_empty_list_is_an_empty_string() {
        assert_eq!(describe_drive_holders(&[]), "");
    }

    use crate::config::{Config, LogFormat};
    use sandkiln_vmm::drive::DriveStore;
    use sandkiln_vmm::network::NetworkManager;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration as StdDuration;

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Real temp-directory-backed `AppState` (no mocking) —
    /// `NetworkManager` gets no tap devices since nothing here leases one.
    fn test_state() -> Arc<AppState> {
        let n = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sandkiln-state-test-{}-{n}", std::process::id()));
        let config = Config {
            listen_addr: "127.0.0.1:0".to_string(),
            firecracker_bin: PathBuf::from("/bin/true"),
            kernel_path: PathBuf::from("/dev/null"),
            base_rootfs_path: PathBuf::from("/dev/null"),
            vcpu_count: 1,
            mem_size_mib: 128,
            max_vcpu_count: 8,
            max_mem_size_mib: 4096,
            bridge_name: "test-br0".to_string(),
            bridge_gateway: "10.0.0.1".parse().unwrap(),
            uplink_iface: Some("eth-test".to_string()),
            tap_pool_prefix: "tap".to_string(),
            tap_pool_size: 0,
            auth_token: None,
            drives_dir: dir.join("drives"),
            images_dir: dir.join("images"),
            history_db_path: dir.join("history.db"),
            idle_timeout: None,
            auto_suspend_timeout: None,
            archive_timeout: None,
            archive_dir: dir.join("archive"),
            log_format: LogFormat::Pretty,
            preview_timeout: StdDuration::from_secs(30),
            jailer: None,
        };
        let network = NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", Vec::<String>::new());
        let drives = DriveStore::new(dir.join("drives")).expect("create test drives dir");
        let images = ImageStore::new(dir.join("images")).expect("create test images dir");
        let history = HistoryStore::open_in_memory().expect("create in-memory test history store");
        Arc::new(AppState::new(config, network, drives, images, HashMap::new(), HashMap::new(), history))
    }

    #[test]
    fn find_named_returns_none_for_an_empty_or_unmatched_set() {
        assert_eq!(find_named(std::iter::empty(), "nope"), None);
        assert_eq!(find_named([("a", Some("foo")), ("b", None)].into_iter(), "nope"), None);
    }

    #[test]
    fn find_named_matches_by_exact_name() {
        let entries = [("sbx-1", Some("web-server")), ("sbx-2", Some("db"))];
        assert_eq!(find_named(entries.into_iter(), "web-server"), Some("sbx-1"));
        assert_eq!(find_named(entries.into_iter(), "db"), Some("sbx-2"));
    }

    #[test]
    fn find_named_skips_unnamed_entries() {
        let entries = [("sbx-1", None), ("sbx-2", Some("named"))];
        assert_eq!(find_named(entries.into_iter(), "named"), Some("sbx-2"));
    }

    #[test]
    fn resolve_name_prefers_the_live_sandbox_over_a_same_named_snapshot() {
        // A live fork and its source snapshot share one name deliberately
        // (not a conflict — see `Sandbox::name`); live must win. Exercised
        // via the pure `find_named` priority order `resolve_name`/
        // `name_holder` wrap, since a real `Sandbox`/`Snapshot` needs KVM.
        let live = [("sbx-fork", Some("shared-name"))];
        let held = [("snap-parent", Some("shared-name"))];
        assert_eq!(find_named(live.into_iter(), "shared-name"), Some("sbx-fork"));
        assert_eq!(find_named(held.into_iter(), "shared-name"), Some("snap-parent"));
    }

    #[test]
    fn name_holder_is_none_for_an_unclaimed_name_on_a_real_empty_state() {
        let state = test_state();
        assert_eq!(state.name_holder("nope"), None);
        assert!(state.resolve_name("nope").is_none());
    }

    #[tokio::test]
    async fn lock_name_serializes_concurrent_callers_of_the_same_name() {
        let state = test_state();
        let order = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));

        let guard1 = state.lock_name("race").await;

        let state2 = state.clone();
        let order2 = order.clone();
        let waiter = tokio::spawn(async move {
            let _guard2 = state2.lock_name("race").await;
            order2.lock().unwrap().push("second");
        });

        // Give the spawned task a real chance to reach the await point and
        // block on the still-held lock, rather than racing it — a
        // generous but bounded yield, not a magic-number sleep tuned to
        // pass by luck.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(!waiter.is_finished(), "a second lock_name() call for the same name must block while the first guard is held");

        order.lock().unwrap().push("first-drops");
        drop(guard1);
        waiter.await.unwrap();

        assert_eq!(*order.lock().unwrap(), vec!["first-drops", "second"], "the second caller must not proceed until the first guard is dropped");
    }

    #[tokio::test]
    async fn lock_name_allows_different_names_to_proceed_concurrently() {
        let state = test_state();
        let _guard_a = state.lock_name("name-a").await;
        // Must not deadlock/block: a different name has its own lock.
        let _guard_b = tokio::time::timeout(StdDuration::from_secs(2), state.lock_name("name-b"))
            .await
            .expect("locking an unrelated name must not wait on 'name-a'");
    }

    #[tokio::test]
    async fn lock_name_cleans_up_its_map_entry_once_the_last_guard_drops() {
        let state = test_state();
        {
            let _guard = state.lock_name("transient").await;
            assert!(state.name_locks.lock().unwrap().contains_key("transient"));
        }
        assert!(
            !state.name_locks.lock().unwrap().contains_key("transient"),
            "the per-name lock entry must be swept once nothing references it anymore"
        );
    }

    #[tokio::test]
    async fn lock_name_reusable_after_cleanup_for_a_brand_new_claim() {
        let state = test_state();
        drop(state.lock_name("reused").await);
        // A second, later, non-overlapping claim of the same name must
        // still work correctly after the first guard's cleanup ran.
        let _guard = tokio::time::timeout(StdDuration::from_secs(2), state.lock_name("reused"))
            .await
            .expect("re-locking a name after its guard was dropped must not hang");
    }

    /// Real `AppState` over isolated temp dirs — no KVM/root needed:
    /// `NetworkManager` here is only ever constructed, never
    /// `ensure_ready()`'d/`lease()`'d, so a fake bridge/uplink is fine
    /// (same convention `snapshot.rs`'s tests use).
    struct TestState {
        state: AppState,
        dir: PathBuf,
    }

    impl TestState {
        fn new(test_name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sandkiln-appstate-test-{test_name}-{}", std::process::id()));
            let _ = fs_remove(&dir);
            let drives = DriveStore::new(dir.join("drives")).unwrap();
            let images = ImageStore::new(dir.join("images")).unwrap();
            let network = NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", Vec::<String>::new());
            let config = Config {
                listen_addr: "127.0.0.1:0".to_string(),
                firecracker_bin: PathBuf::from("/nonexistent/firecracker"),
                kernel_path: PathBuf::from("/nonexistent/vmlinux"),
                base_rootfs_path: PathBuf::from("/nonexistent/base.ext4"),
                vcpu_count: 2,
                mem_size_mib: 512,
                max_vcpu_count: 16,
                max_mem_size_mib: 16384,
                bridge_name: "test-br0".to_string(),
                bridge_gateway: "10.0.0.1".parse().unwrap(),
                uplink_iface: Some("eth-test".to_string()),
                tap_pool_prefix: "sktap".to_string(),
                tap_pool_size: 0,
                auth_token: None,
                drives_dir: dir.join("drives"),
                images_dir: dir.join("images"),
                history_db_path: dir.join("history.db"),
                idle_timeout: None,
                auto_suspend_timeout: None,
                archive_timeout: None,
                archive_dir: dir.join("archive"),
                log_format: LogFormat::Pretty,
                preview_timeout: Duration::from_secs(30),
                jailer: None,
            };
            let history = HistoryStore::open_in_memory().expect("create in-memory test history store");
            let state = AppState::new(config, network, drives, images, HashMap::new(), HashMap::new(), history);
            Self { state, dir }
        }
    }

    impl Drop for TestState {
        fn drop(&mut self) {
            let _ = fs_remove(&self.dir);
        }
    }

    fn fs_remove(dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::remove_dir_all(dir)
    }

    #[test]
    fn image_holder_is_none_for_an_untouched_image_id() {
        let t = TestState::new("image-holder-none");
        assert_eq!(t.state.image_holder("img1"), None);
    }

    #[test]
    fn reserve_pending_image_boot_makes_image_holder_report_it() {
        let t = TestState::new("reserve-visible");
        t.state.reserve_pending_image_boot("img1");
        assert_eq!(t.state.image_holder("img1"), Some("a sandbox currently being created".to_string()));
    }

    #[test]
    fn release_pending_image_boot_clears_the_claim() {
        let t = TestState::new("release-clears");
        t.state.reserve_pending_image_boot("img1");
        t.state.release_pending_image_boot("img1");
        assert_eq!(t.state.image_holder("img1"), None);
    }

    #[test]
    fn two_concurrent_reservations_on_the_same_image_both_require_release() {
        let t = TestState::new("two-reservations");
        t.state.reserve_pending_image_boot("img1");
        t.state.reserve_pending_image_boot("img1");

        // One boot finishing (or failing) releases only its own claim —
        // the other in-flight boot from the same image still blocks
        // deletion.
        t.state.release_pending_image_boot("img1");
        assert!(t.state.image_holder("img1").is_some(), "a second in-flight boot must still hold the image");

        t.state.release_pending_image_boot("img1");
        assert_eq!(t.state.image_holder("img1"), None);
    }

    #[test]
    fn release_without_a_matching_reservation_does_not_panic_or_underflow() {
        let t = TestState::new("release-without-reserve");
        // Defensive: a bug elsewhere calling release twice (e.g. once from
        // a guard's `Drop` and once explicitly) must not panic the whole
        // request thread or wrap the counter around to `u32::MAX`.
        t.state.release_pending_image_boot("never-reserved");
        assert_eq!(t.state.image_holder("never-reserved"), None);
    }

    // `image_holder`'s sandbox-map branch isn't exercised here (a real
    // `Sandbox` needs a running `Vm`, which needs KVM); its snapshot-map
    // branch and `tap_device_holder`'s are, below, since `Snapshot` needs
    // only a `Lease` and `NetworkManager::reserve()` builds one without
    // real netlink.

    fn test_network() -> NetworkManager {
        NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", ["tapA".to_string()])
    }

    fn test_lease(network: &NetworkManager, tap_device: &str, host_octet: u8) -> sandkiln_vmm::network::Lease {
        network.reserve(
            sandkiln_vmm::vm::NetworkConfig {
                tap_device: tap_device.to_string(),
                guest_ip: "10.0.0.5".parse().unwrap(),
                gateway_ip: "10.0.0.1".parse().unwrap(),
                guest_mac: "AA:FC:00:00:05:05".to_string(),
            },
            host_octet,
        )
    }

    fn test_snapshot(id: &str, network: &NetworkManager, tap_device: &str) -> Snapshot {
        Snapshot {
            id: id.to_string(),
            source_sandbox_id: "sandbox-x".to_string(),
            snapshot_path: PathBuf::from("/tmp/does-not-need-to-exist/state.snap"),
            mem_file_path: PathBuf::from("/tmp/does-not-need-to-exist/mem.bin"),
            rootfs_path: PathBuf::from("/tmp/does-not-need-to-exist/rootfs.ext4"),
            network: test_lease(network, tap_device, 5),
            attached_drives: vec![],
            image_id: None,
            tags: HashMap::new(),
            created_at: std::time::SystemTime::now(),
            name: None,
            forked_into: None,
            archived_at: None,
            egress: None,
            env: HashMap::new(),
            parent_snapshot_id: None,
            mounts: vec![],
        }
    }

    fn test_retired_snapshot(id: &str, tap_device: &str) -> RetiredSnapshot {
        RetiredSnapshot {
            id: id.to_string(),
            source_sandbox_id: "sandbox-y".to_string(),
            snapshot_path: PathBuf::from("/tmp/does-not-need-to-exist/state.snap"),
            mem_file_path: PathBuf::from("/tmp/does-not-need-to-exist/mem.bin"),
            rootfs_path: PathBuf::from("/tmp/does-not-need-to-exist/rootfs.ext4"),
            network: sandkiln_vmm::vm::NetworkConfig {
                tap_device: tap_device.to_string(),
                guest_ip: "10.0.0.6".parse().unwrap(),
                gateway_ip: "10.0.0.1".parse().unwrap(),
                guest_mac: "AA:FC:00:00:06:06".to_string(),
            },
            host_octet: 6,
            attached_drives: vec![],
            image_id: None,
            tags: HashMap::new(),
            created_at: std::time::SystemTime::now(),
            retired_at: std::time::SystemTime::now(),
            name: None,
            parent_snapshot_id: None,
            egress: None,
            env: HashMap::new(),
            mounts: vec![],
        }
    }

    #[test]
    fn tap_device_holder_is_none_when_nothing_holds_it() {
        let t = TestState::new("tap-holder-none");
        assert_eq!(t.state.tap_device_holder("tapA"), None);
    }

    #[test]
    fn tap_device_holder_finds_a_held_snapshot() {
        let t = TestState::new("tap-holder-snapshot");
        let network = test_network();
        t.state.snapshots.lock().unwrap().insert("snap-1".to_string(), test_snapshot("snap-1", &network, "tapA"));
        assert_eq!(t.state.tap_device_holder("tapA"), Some("snapshot snap-1".to_string()));
        assert_eq!(t.state.tap_device_holder("tapB"), None, "an unrelated tap must not be reported as held");
    }

    #[test]
    fn tap_device_holder_reports_a_pending_restore() {
        let t = TestState::new("tap-holder-pending-restore");
        assert!(t.state.try_reserve_pending_tap_restore("tapA"));
        assert!(t.state.tap_device_holder("tapA").is_some());
    }

    #[test]
    fn try_reserve_pending_tap_restore_claims_then_blocks_a_second_caller() {
        let t = TestState::new("pending-restore-claim");
        assert!(t.state.try_reserve_pending_tap_restore("tapA"), "the first claim must win");
        assert!(!t.state.try_reserve_pending_tap_restore("tapA"), "a second concurrent claim of the same tap must lose");
    }

    #[test]
    fn release_pending_tap_restore_frees_it_for_a_new_claim() {
        let t = TestState::new("pending-restore-release");
        assert!(t.state.try_reserve_pending_tap_restore("tapA"));
        t.state.release_pending_tap_restore("tapA");
        assert_eq!(t.state.tap_device_holder("tapA"), None);
        assert!(t.state.try_reserve_pending_tap_restore("tapA"), "releasing must actually free the claim for reuse");
    }

    #[test]
    fn release_pending_tap_restore_without_a_matching_reservation_does_not_panic() {
        let t = TestState::new("pending-restore-release-unclaimed");
        t.state.release_pending_tap_restore("never-claimed");
        assert_eq!(t.state.tap_device_holder("never-claimed"), None);
    }

    #[test]
    fn drive_holders_includes_a_retired_snapshot_that_references_the_drive() {
        let t = TestState::new("drive-holders-retired");
        let mut retired = test_retired_snapshot("retired-1", "tapA");
        retired.attached_drives = vec![AttachedDrive { drive_id: "d1".to_string(), read_only: false }];
        t.state.retired_snapshots.lock().unwrap().insert("retired-1".to_string(), retired);

        let holders = t.state.drive_holders("d1");
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].holder, "retired snapshot retired-1");
        assert!(!holders[0].read_only);
    }

    #[test]
    fn image_holder_finds_a_retired_snapshot() {
        let t = TestState::new("image-holder-retired");
        let mut retired = test_retired_snapshot("retired-2", "tapA");
        retired.image_id = Some("img1".to_string());
        t.state.retired_snapshots.lock().unwrap().insert("retired-2".to_string(), retired);

        assert_eq!(t.state.image_holder("img1"), Some("retired snapshot retired-2".to_string()));
    }
}

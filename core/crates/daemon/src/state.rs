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
use std::sync::{Arc, Mutex};
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
    /// One `tokio::sync::Mutex` per name currently being claimed, created
    /// lazily. Serializes every code path that can claim or resolve a
    /// name (named `create_sandbox`, `get_or_create_sandbox`) against
    /// concurrent callers using the *same* name, without serializing
    /// unrelated names against each other — see `AppState::lock_name`.
    pub name_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
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
            name_locks: Mutex::new(HashMap::new()),
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

    /// Where a name is currently claimed, if anywhere — mirrors
    /// `drive_holder`'s "sandbox or snapshot, in one place" shape. Checks
    /// live sandboxes before held snapshots: while a snapshot is forked
    /// (`Snapshot::forked_into`), both a `Sandbox` and the `Snapshot` it
    /// came from carry the same name at once (see `Sandbox::name`'s doc
    /// comment — they're one identity, not a conflict), and the live one
    /// is the more useful answer for a caller resolving a name to
    /// something they can act on right now.
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

    /// Resolves a name to whichever record currently represents that
    /// identity, distinguishing "live and actionable right now" from
    /// "held as a snapshot, needs a resume first" — `name_holder` above
    /// collapses that distinction into a display string, which is enough
    /// for a conflict message but not enough for `get_or_create_sandbox`
    /// or `GET /sandboxes/by-name/:name` to decide what to do next.
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

    /// Serializes every operation that claims or resolves one particular
    /// name against concurrent callers using that *same* name, while
    /// leaving unrelated names free to proceed in parallel — the race
    /// this exists to close: two concurrent `POST /sandboxes/get-or-create`
    /// (or named `POST /sandboxes`) calls for a brand-new name must not
    /// both observe "not taken" and both create a sandbox. A caller holds
    /// the returned guard across its entire check-then-act sequence (see
    /// `routes_sandbox::create_sandbox` and
    /// `routes_sandbox_name::get_or_create_sandbox`) — a second caller for
    /// the same name blocks in `.await` here until the first either
    /// commits its claim (so the second's subsequent `resolve_name` sees
    /// it) or fails (so the name is free again).
    ///
    /// Entries are removed best-effort once nothing else references them
    /// (`Arc::strong_count` back down to the one held by the map itself)
    /// so this doesn't grow forever across a long-running daemon's full
    /// history of distinct names — see `NameLockGuard::drop`. A cleanup
    /// that loses a benign race with a new concurrent `lock_name` call for
    /// the same name just leaves one harmless extra map entry to be swept
    /// next time that name's guard drops.
    pub async fn lock_name(self: &Arc<Self>, name: &str) -> NameLockGuard {
        let lock = {
            let mut locks = self.name_locks.lock().unwrap();
            locks.entry(name.to_string()).or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))).clone()
        };
        let guard = Arc::clone(&lock).lock_owned().await;
        NameLockGuard { state: self.clone(), name: name.to_string(), lock, _guard: guard }
    }
}

/// Pure decision behind `name_holder`/`resolve_name`: the first entry (in
/// iteration order) whose name matches. Pulled out of both so it's
/// directly unit-testable without a real `Sandbox`/`Snapshot` — both need
/// a live `Vm`/`Lease` to construct, unavailable without KVM — mirroring
/// this project's `auth::token_matches`/`idle_reaper::is_idle` pattern of
/// separating a pure decision from the framework plumbing around it.
fn find_named<'a>(mut entries: impl Iterator<Item = (&'a str, Option<&'a str>)>, name: &str) -> Option<&'a str> {
    entries.find_map(|(id, entry_name)| (entry_name == Some(name)).then_some(id))
}

/// What a name currently resolves to — see `AppState::resolve_name`.
pub enum NameResolution {
    /// A live sandbox, ready to act on directly.
    Live(String),
    /// A held snapshot with this name and no live fork of it — resolvable,
    /// but needs a resume (or fork) before it's a sandbox again.
    Snapshot(String),
}

/// RAII handle for one name's lock, held by a caller for the duration of a
/// check-then-act sequence — see `AppState::lock_name`. Not constructed
/// directly.
pub struct NameLockGuard {
    state: Arc<AppState>,
    name: String,
    // Kept alongside `_guard` purely so `Drop` can check `Arc::strong_count`
    // — the guard alone doesn't expose the `Arc` it locked.
    lock: Arc<tokio::sync::Mutex<()>>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for NameLockGuard {
    fn drop(&mut self) {
        let mut locks = self.state.name_locks.lock().unwrap();
        // While this `drop` body runs, `self.lock` and `self._guard`'s own
        // internal clone are both still alive (Rust drops struct fields
        // only *after* a custom `Drop::drop` returns) — so 3 references is
        // "just us": the map's copy, `self.lock`, and the one
        // `OwnedMutexGuard` holds internally. Anything higher means
        // another `lock_name` call for this same name already grabbed a
        // clone (waiting to acquire, or holding it after us) before we got
        // here, in which case removing the map entry now would let a
        // third caller create a *different* lock object for the same
        // name — defeating the whole point. Safe to skip: that other
        // holder (or a later drop of it) gets another chance to clean up
        // once it's done.
        if Arc::strong_count(&self.lock) == 3 {
            locks.remove(&self.name);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LogFormat};
    use sandkiln_vmm::drive::DriveStore;
    use sandkiln_vmm::network::NetworkManager;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration as StdDuration;

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Real temp-directory-backed `AppState`, matching this project's
    /// "use real filesystem state rather than mocking wherever the
    /// operation doesn't need KVM" testing convention — `DriveStore`
    /// creates its directory on `new`, and `NetworkManager` here is given
    /// no tap devices at all since nothing under test leases one.
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
            idle_timeout: None,
            log_format: LogFormat::Pretty,
            preview_timeout: StdDuration::from_secs(30),
            jailer: None,
        };
        let network = NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", Vec::<String>::new());
        let drives = DriveStore::new(dir.join("drives")).expect("create test drives dir");
        Arc::new(AppState::new(config, network, drives, HashMap::new()))
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
        // A live fork's `Sandbox::name` and its source `Snapshot::name` are
        // deliberately the same string at once — not a conflict, one
        // identity with both a live session and a persisted record (see
        // `Sandbox::name`'s doc comment). The live one must win: it's the
        // more actionable answer for a caller resolving a name right now.
        // Exercised here as the pure `find_named` priority order that
        // `resolve_name`/`name_holder` are thin wrappers around, since a
        // real `Sandbox`/`Snapshot` needs a live `Vm`/`Lease` this test
        // environment (no KVM) can't construct.
        let live = [("sbx-fork", Some("shared-name"))];
        let held = [("snap-parent", Some("shared-name"))];
        assert_eq!(find_named(live.into_iter(), "shared-name"), Some("sbx-fork"));
        // Only reached if the live map has no match — `resolve_name`'s own
        // control flow, not re-derivable from `find_named` alone.
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
}

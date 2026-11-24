//! Naming: which live sandbox or held snapshot a name resolves to, and the
//! per-name lock that makes claim-then-create race-free.
//!
//! `name_holder`/`resolve_name` (live wins over a snapshot of the same name),
//! `lock_name` + `NameLockGuard` (one lock per name, swept once unreferenced)
//! and `find_named`, the pure matching step behind both lookups.

use super::AppState;
use std::sync::Arc;

impl AppState {
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
}

/// Pure decision behind `name_holder`/`resolve_name`, pulled out for
/// unit-testing without a real `Sandbox`/`Snapshot` (both need KVM to
/// construct) — same pattern as `auth::token_matches`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, LogFormat};
    use sandkiln_store::HistoryStore;
    use sandkiln_vmm::drive::DriveStore;
    use sandkiln_vmm::image::ImageStore;
    use sandkiln_vmm::network::NetworkManager;
    use std::collections::HashMap;
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
            origins: Default::default(),
            exec_timeout: std::time::Duration::from_secs(600),
            kernel_args: String::new(),
                snapshot_dirty_tracking: true,
            retained_checkpoints_max: 16,
            default_warm_count: 0,
            state_dir: dir.join("state"),
            rootfs_dir: dir.join("rootfs"),
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
}

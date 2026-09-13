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

/// One drive attached to a sandbox or a held snapshot, and whether it was
/// attached read-only. `Sandbox::attached_drives` and
/// `Snapshot::attached_drives` both carry this rather than a bare drive
/// id, because whether a *new* attach may coexist with the existing ones
/// depends on both pieces of information — see `can_attach_read_only`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedDrive {
    pub drive_id: String,
    pub read_only: bool,
}

pub struct AppState {
    pub config: Config,
    pub network: NetworkManager,
    pub drives: DriveStore,
    pub images: ImageStore,
    /// `Some` exactly when `config.jailer` is `Some` — the pool of
    /// uid/gid pairs `routes_sandbox::create_sandbox` leases from for
    /// each jailed boot, released back on `stop_sandbox_by_id`. Built
    /// here rather than passed in separately since its range comes
    /// straight out of `config.jailer`.
    pub jailer_ids: Option<JailerIdPool>,
    pub sandboxes: Mutex<HashMap<String, Sandbox>>,
    pub snapshots: Mutex<HashMap<String, Snapshot>>,
    /// Retired checkpoints — see `crate::snapshot_history`'s module doc
    /// comment for the full "time-travel restore" design. Deliberately a
    /// separate map from `snapshots` above, not a flag on `Snapshot`
    /// itself: a retired checkpoint holds no live `Lease` at all (nothing
    /// reserved out of `NetworkManager`'s free pool), a structurally
    /// different resource-ownership state from every entry in
    /// `snapshots`, which always holds one for as long as it exists.
    pub retired_snapshots: Mutex<HashMap<String, RetiredSnapshot>>,
    /// One `tokio::sync::Mutex` per name currently being claimed, created
    /// lazily. Serializes every code path that can claim or resolve a
    /// name (named `create_sandbox`, `get_or_create_sandbox`) against
    /// concurrent callers using the *same* name, without serializing
    /// unrelated names against each other — see `AppState::lock_name`.
    pub name_locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Refcounted claims on an image id that's being cloned into a
    /// not-yet-registered `Sandbox` right now (`routes_sandbox::create_sandbox`
    /// holds one for the duration of the boot). Closes the race between
    /// "checked the image exists" and "the new sandbox is actually visible
    /// in `sandboxes`, so `image_holder` can see it there instead": without
    /// this, `DELETE /images/:id` could succeed in the gap while a boot
    /// from that exact image is still in flight, deleting the file out
    /// from under an in-progress `cp`. Keyed by image id rather than a
    /// single bool per id because two sandboxes can legitimately boot from
    /// the same image concurrently.
    pending_image_boots: Mutex<HashMap<String, u32>>,
    pub metrics: Metrics,
    /// Reused across every `/preview` proxy request rather than built
    /// per-request, so repeated hits on one dev server benefit from
    /// `hyper-util`'s connection pooling instead of a fresh TCP handshake
    /// (and, for a WebSocket-using dev server later, from a client
    /// already wired for keep-alive) every time.
    pub preview_client: PreviewClient,
    /// Durable sandbox-lifecycle history (`sandkiln-store`) — a
    /// completely separate concern from `sandboxes`/`snapshots` above;
    /// see that crate's own module doc comment for exactly what it does
    /// and does not solve.
    pub history: HistoryStore,
    /// Configured pre-warmed pools, keyed by their caller-given id — see
    /// `crate::pool`'s module doc comment. In-memory only, deliberately
    /// (not durable across a restart, unlike `snapshots` above) — see
    /// that module for why.
    pub pools: Mutex<HashMap<String, Pool>>,
    /// Tap devices with a `POST /snapshots/history/:id/restore` currently
    /// in flight against them — closes a race `tap_device_holder` alone
    /// can't: two *different* retired checkpoints in the same lineage
    /// share the same frozen tap device/IP/MAC (see
    /// `crate::snapshot_history`'s module doc comment), so concurrently
    /// restoring two different retired ids for that one device would both
    /// pass a plain "is this live or held right now" check (neither is,
    /// yet) and then race `NetworkManager::reserve`, which does not
    /// itself detect a double reservation — it exists for the
    /// startup-reconcile case, where by construction nothing else could
    /// be live yet. Same shape as `pending_image_boots` above, but a
    /// plain set rather than a refcount: unlike booting from a shared
    /// image, only one restore of a given tap device may ever be in
    /// flight at a time.
    pending_tap_restores: Mutex<std::collections::HashSet<String>>,
}

impl AppState {
    /// `snapshots` is the result of `crate::snapshot::reconcile` run
    /// against the on-disk snapshot store before this is called — passed
    /// in rather than always starting empty so a daemon restart doesn't
    /// silently orphan every snapshot that was durable on disk (see
    /// `main.rs`).
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
        }
    }

    /// Every current holder of `drive_id` — running sandboxes and held
    /// snapshots with it frozen into saved state (Firecracker bakes a
    /// drive's host path into the snapshot the same way it does network
    /// config, so a snapshotted drive is still "in use" even though no
    /// `Vm` is running) — each labeled and marked with whether that
    /// particular attachment is read-only. Empty means nothing holds it.
    ///
    /// Checked wherever an operation would conflict with the drive still
    /// being held: attaching it to another sandbox (via
    /// `can_attach_read_only`, since many simultaneous read-only holders
    /// are fine) or deleting it outright (never fine while this is
    /// non-empty, regardless of read-only status).
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
        // A retired checkpoint (`crate::snapshot_history`) references its
        // drives exactly as durably as a held `Snapshot` does — it's just
        // sitting in history instead of `snapshots` right now, not any
        // less real a reference. Without this, a drive a retired
        // checkpoint depends on read-write could be silently re-attached
        // read-write elsewhere, and restoring that checkpoint later would
        // hand two live VMs the same mutable backing file.
        holders.extend(self.retired_snapshots.lock().unwrap().values().filter_map(|s| {
            s.attached_drives
                .iter()
                .find(|d| d.drive_id == drive_id)
                .map(|d| DriveHold { holder: format!("retired snapshot {}", s.id), read_only: d.read_only })
        }));
        holders
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

    /// Where an image id is currently referenced, if anywhere — mirrors
    /// `drive_holder`, but for `sandkiln_vmm::image::ImageStore` entries.
    /// Checked before a sandbox boot commits to an image id and before
    /// `DELETE /images/:id`. Also covers a boot currently in flight from
    /// this image (`pending_image_boots`) — a sandbox isn't inserted into
    /// `sandboxes` until its `Vm` has actually booted, but the image is
    /// already "in use" for deletion purposes from the moment the boot
    /// starts, not just once the sandbox is visible.
    pub fn image_holder(&self, image_id: &str) -> Option<String> {
        if self.pending_image_boots.lock().unwrap().get(image_id).is_some_and(|count| *count > 0) {
            return Some("a sandbox currently being created".to_string());
        }
        if let Some(sandbox) = self.sandboxes.lock().unwrap().values().find(|s| s.image_id.as_deref() == Some(image_id))
        {
            return Some(format!("sandbox {}", sandbox.id));
        }
        if let Some(snapshot) =
            self.snapshots.lock().unwrap().values().find(|s| s.image_id.as_deref() == Some(image_id))
        {
            return Some(format!("snapshot {}", snapshot.id));
        }
        // Same reasoning as `drive_holders`' retired-checkpoint extension
        // just above: a retired checkpoint's rootfs was cloned from this
        // image at the sandbox's original boot, same as a held snapshot's
        // was — deleting the image out from under it would only matter if
        // that checkpoint is ever restored, but the check has to happen
        // now, not deferred to restore time.
        if let Some(retired) =
            self.retired_snapshots.lock().unwrap().values().find(|s| s.image_id.as_deref() == Some(image_id))
        {
            return Some(format!("retired snapshot {}", retired.id));
        }
        None
    }

    /// Where the tap device backing `tap_device` is currently held, if
    /// anywhere — a live sandbox's own lease, a held snapshot's (whether
    /// or not it's currently lent out to a live fork: the `Lease` stays
    /// inside the `Snapshot` the whole time either way, see
    /// `Snapshot::forked_into`'s doc comment), or another retired
    /// checkpoint that's *itself* mid-restore right now. `None` means
    /// free to reserve.
    ///
    /// This is the check `routes_snapshot_history::restore_snapshot_history`
    /// needs before it can safely call `NetworkManager::reserve` for a
    /// retired checkpoint: unlike `snapshot::reconcile`'s call to the same
    /// method (always at startup, before anything else can possibly be
    /// live), a restore can race real, already-live users of this exact
    /// tap device — and `NetworkManager::reserve` itself does not check
    /// for that; it exists for the startup case, where by construction
    /// nothing else could be holding it yet, and will silently proceed
    /// (with only a warning) even if the tap it's given is already
    /// checked out. This is the guard that makes calling it safe outside
    /// that one narrow startup circumstance.
    pub fn tap_device_holder(&self, tap_device: &str) -> Option<String> {
        if let Some(sandbox) = self
            .sandboxes
            .lock()
            .unwrap()
            .values()
            .find(|s| s.network.as_ref().is_some_and(|n| n.config.tap_device == tap_device))
        {
            return Some(format!("sandbox {}", sandbox.id));
        }
        if let Some(snapshot) =
            self.snapshots.lock().unwrap().values().find(|s| s.network.config.tap_device == tap_device)
        {
            return Some(format!("snapshot {}", snapshot.id));
        }
        if self.pending_tap_restores.lock().unwrap().contains(tap_device) {
            return Some("another restore already in progress for this checkpoint's network identity".to_string());
        }
        None
    }

    /// Attempts to claim `tap_device` for an in-flight restore —
    /// `true` if this call won the claim (the caller may proceed),
    /// `false` if another restore already holds it (the caller must
    /// refuse, not proceed) — see `pending_tap_restores`'s own doc
    /// comment for the race this closes. Paired with
    /// `release_pending_tap_restore`, ideally via an RAII guard (see
    /// `routes_snapshot_history::PendingTapRestoreGuard`) so every exit
    /// path — success, failure, or a panic unwind — releases it.
    pub fn try_reserve_pending_tap_restore(&self, tap_device: &str) -> bool {
        self.pending_tap_restores.lock().unwrap().insert(tap_device.to_string())
    }

    pub fn release_pending_tap_restore(&self, tap_device: &str) {
        self.pending_tap_restores.lock().unwrap().remove(tap_device);
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

    /// Claims a pending reference on `image_id` for the duration of a
    /// sandbox boot from it — see `pending_image_boots`'s doc comment.
    /// Paired with `release_pending_image_boot`, ideally via an RAII guard
    /// at the call site so it's released on every exit path, not just the
    /// success one (see `routes_sandbox::ImagePendingBootGuard`).
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

/// One thing currently holding a drive (a running sandbox or a held
/// snapshot), and whether it holds it read-only. See `AppState::drive_holders`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveHold {
    pub holder: String,
    pub read_only: bool,
}

/// The multi-holder rule at the heart of this feature: a drive may be
/// attached to arbitrarily many holders at once, but only if every
/// existing holder *and* the new attach being requested are all
/// read-only. A single read-write attachment — existing or requested —
/// needs exclusive, single-holder access, exactly like every attachment
/// did before read-only sharing existed. Pulled out of
/// `AppState::drive_holders`'s callers so it's directly testable without
/// `AppState`, a mutex, or axum.
pub fn can_attach_read_only(existing: &[bool], requesting_read_only: bool) -> bool {
    existing.is_empty() || (requesting_read_only && existing.iter().all(|ro| *ro))
}

/// Renders a list of `DriveHold`s into the human-readable form used in
/// `AppError::Conflict` messages and nowhere else — kept next to
/// `DriveHold` rather than duplicated at each call site.
pub fn describe_drive_holders(holders: &[DriveHold]) -> String {
    holders
        .iter()
        .map(|h| if h.read_only { format!("{} (read-only)", h.holder) } else { h.holder.clone() })
        .collect::<Vec<_>>()
        .join(", ")
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

    /// Builds a real `AppState` against real, isolated temp directories —
    /// no KVM/root needed for anything exercised here: `NetworkManager` is
    /// only ever constructed, never `ensure_ready()`'d or `lease()`'d (both
    /// need real netlink access), so a fake bridge/uplink name is fine, the
    /// same convention `snapshot.rs`'s tests already use.
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

    // `image_holder`'s sandbox-map branch isn't exercised here: a real
    // `Sandbox` needs a real `sandkiln_vmm::vm::Vm` (a running Firecracker
    // process/vsock connection), which needs KVM. Its snapshot-map branch
    // -- and `tap_device_holder`'s below -- *are* covered further down:
    // unlike `Sandbox`, `Snapshot` needs only a `Lease`, and
    // `NetworkManager::reserve()` builds one without any real netlink
    // call at all (see `test_network`/`test_lease` below) -- the same
    // reason `snapshot.rs`'s own tests can construct a real `Snapshot`
    // without KVM either.

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
            parent_snapshot_id: None,
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

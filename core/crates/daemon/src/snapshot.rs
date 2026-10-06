use crate::state::{AttachedDrive, Mount};
use sandkiln_vmm::egress::EgressPolicy;
use sandkiln_vmm::network::{Lease, NetworkManager};
use sandkiln_vmm::vm::NetworkConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A saved, stopped microVM: memory and device state on disk, resumable
/// into a new live sandbox. What a sandbox becomes instead of being fully
/// torn down.
pub struct Snapshot {
    pub id: String,
    /// Purely informational — that sandbox no longer exists once a
    /// `Snapshot` does.
    pub source_sandbox_id: String,
    /// State file written by `Vm::snapshot`.
    pub snapshot_path: PathBuf,
    /// Guest-memory file written by `Vm::snapshot`.
    pub mem_file_path: PathBuf,
    /// Carried over unmodified. Firecracker's snapshot records only this
    /// file's host path, not its contents, so it must stay put until
    /// resume (ownership passes to the new sandbox) or deletion (file
    /// removed).
    pub rootfs_path: PathBuf,
    /// Held, not released to `NetworkManager`'s pool: guest IP/MAC are
    /// frozen into the snapshotted memory image via boot-time kernel args,
    /// so a resume must reattach this exact tap device, never a fresh
    /// lease. See `Vm::resume`.
    pub network: Lease,
    /// Carried over from the source sandbox, read-only flag included — a
    /// drive's data lives inside the snapshotted state the same way
    /// network config does, so attaching a read-write copy elsewhere
    /// while this snapshot still holds it read-write would let two VMs
    /// write one file.
    pub attached_drives: Vec<AttachedDrive>,
    /// Carried over from `Sandbox::image_id`; `None` = daemon default
    /// rootfs, same meaning as there.
    pub image_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Carried over from `Sandbox::name`, if set — lets a caller find
    /// this snapshot again by name via `GET /sandboxes/by-name/:name`
    /// (once resumed) or `get-or-create`.
    pub name: Option<String>,
    /// Live sandbox currently forked from this snapshot without consuming
    /// it, if any. A resume reopens the exact rootfs file and (if
    /// networked) the exact tap device this snapshot records, both frozen
    /// at the source's original boot — a second live descendant sharing
    /// either means two Firecracker processes writing one rootfs file, or
    /// two guests presenting the same IP/MAC on the bridge at once. This
    /// field is the lock ruling both out: set while a fork is resumed,
    /// cleared only once that descendant's `Vm` is killed
    /// (`stop_sandbox_by_id`), checked by `fork_snapshot`/
    /// `resume_snapshot`/`delete_snapshot`/`snapshot_sandbox` alike.
    pub forked_into: Option<String>,
    /// When moved from `snapshots_root()` to `archive_dir`; `None` = still
    /// hot. Set once by `archive_snapshot_by_id`, never cleared — no
    /// un-archive op; resume/fork read from wherever the path fields
    /// currently point, hot or archived, with no code-path difference.
    /// See `idle_reaper`'s archive pass.
    pub archived_at: Option<SystemTime>,
    /// Carried through so resume/fork can re-apply it
    /// (`sandkiln_vmm::egress::apply`) — without this, a protected
    /// sandbox's firewall would silently vanish on snapshot+resume, a
    /// real security regression.
    pub egress: Option<EgressPolicy>,
    /// The snapshot this one's source sandbox was itself resumed/forked
    /// from, if any (`None` = cold-booted, a lineage root). A *parent*
    /// pointer, not a live reference — may point at a since-deleted
    /// snapshot, in which case `GET /snapshots?parent_snapshot_id=` just
    /// ends the chain there. See `ROADMAP.md`'s "Snapshot lineage" entry
    /// for why a walkable pointer, not a fuller ancestry tree, is the
    /// deliberately narrow first slice.
    pub parent_snapshot_id: Option<String>,
    /// Carried over from `Sandbox::mounts` — see `routes_mounts`. No
    /// credentials here, and nothing to re-apply on resume/fork: a mount
    /// is a live guest FUSE process, already captured in the snapshotted
    /// memory image.
    pub mounts: Vec<Mount>,
    /// Carried over from `Sandbox::env`, restored identically on resume
    /// **and** fork — no `egress`-style ownership asymmetry, this is
    /// plain data.
    pub env: HashMap<String, String>,
}

/// On-disk mirror of a `Snapshot` (excluding `state.snap`/`mem.bin`, which
/// live at fixed names under `snapshot_dir(id)`). Written by
/// `Snapshot::persist`, read back by `reconcile` at startup — this file's
/// existence is what makes a `Snapshot` durable across a restart, the
/// same "filesystem is the source of truth" convention
/// `sandkiln_vmm::drive` uses. Every `#[serde(default)]` field below was
/// added after this struct shipped — defaulting lets a `meta.json` from
/// before that field existed still reconcile cleanly instead of failing.
#[derive(Serialize, Deserialize)]
struct SnapshotMeta {
    id: String,
    source_sandbox_id: String,
    rootfs_path: PathBuf,
    tap_device: String,
    guest_ip: Ipv4Addr,
    gateway_ip: Ipv4Addr,
    guest_mac: String,
    host_octet: u8,
    attached_drives: Vec<AttachedDrive>,
    #[serde(default)]
    image_id: Option<String>,
    tags: HashMap<String, String>,
    created_at_unix: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    archived_at_unix: Option<u64>,
    #[serde(default)]
    egress: Option<EgressPolicy>,
    #[serde(default)]
    parent_snapshot_id: Option<String>,
    #[serde(default)]
    mounts: Vec<Mount>,
    #[serde(default)]
    env: HashMap<String, String>,
}

impl Snapshot {
    /// Writes metadata into `dir` atomically (write-then-rename, see
    /// `write_atomically`) so a crash mid-write can't leave `reconcile` a
    /// torn file. `dir` is explicit, not always `snapshot_dir(&self.id)`,
    /// so `archive_snapshot_by_id` can reuse this for the archive root —
    /// the only two call sites, each already knowing which root to use.
    pub fn persist(&self, dir: &Path) -> io::Result<()> {
        let meta = SnapshotMeta {
            id: self.id.clone(),
            source_sandbox_id: self.source_sandbox_id.clone(),
            rootfs_path: self.rootfs_path.clone(),
            tap_device: self.network.config.tap_device.clone(),
            guest_ip: self.network.config.guest_ip,
            gateway_ip: self.network.config.gateway_ip,
            guest_mac: self.network.config.guest_mac.clone(),
            host_octet: self.network.host_octet(),
            attached_drives: self.attached_drives.clone(),
            image_id: self.image_id.clone(),
            tags: self.tags.clone(),
            created_at_unix: self.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            name: self.name.clone(),
            archived_at_unix: self.archived_at.map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()),
            egress: self.egress.clone(),
            parent_snapshot_id: self.parent_snapshot_id.clone(),
            mounts: self.mounts.clone(),
            env: self.env.clone(),
        };
        let json = serde_json::to_vec_pretty(&meta).map_err(|e| io::Error::other(format!("serializing snapshot metadata: {e}")))?;
        write_atomically(&meta_path(dir), &json)
    }
}

/// Per-snapshot directories live under the daemon's temp dir, alongside
/// the loose `sandkiln-rootfs-*.ext4` files `create_sandbox` writes there
/// — same "OS temp dir, daemon-prefixed" convention, no new storage
/// location invented.
pub fn snapshots_root() -> PathBuf {
    std::env::temp_dir().join("sandkiln-snapshots")
}

/// Where one snapshot's state, memory, and metadata files live.
pub fn snapshot_dir(snapshot_id: &str) -> PathBuf {
    snapshots_root().join(snapshot_id)
}

// Shared with `snapshot_history.rs` (`pub(crate)`) -- a retired checkpoint
// uses this same three-filename directory-layout convention, not a
// snapshot-specific one.
pub(crate) fn meta_path(dir: &Path) -> PathBuf {
    dir.join("meta.json")
}

pub(crate) fn state_path(dir: &Path) -> PathBuf {
    dir.join("state.snap")
}

pub(crate) fn mem_path(dir: &Path) -> PathBuf {
    dir.join("mem.bin")
}

/// Same per-snapshot-directory shape as `snapshot_dir`, under
/// `Config::archive_dir` instead. See `archive_snapshot_by_id`.
pub fn archive_snapshot_dir(archive_root: &Path, snapshot_id: &str) -> PathBuf {
    archive_root.join(snapshot_id)
}

/// Moves a snapshot's state and memory files from wherever they
/// currently live into `dest_dir` (created if needed), renaming each to
/// this module's own fixed filenames.
///
/// **Deliberately does not touch `snapshot.rootfs_path` at all.**
/// Firecracker bakes the rootfs backing file's absolute host path into
/// `state.snap` itself with no override at resume time, so moving it
/// breaks every future resume/fork — confirmed by a real resume failure
/// when an earlier version of this function moved it too. See the
/// website's Persistence model architecture page (or `ROADMAP.md`'s
/// "Persistence and snapshotting" section, tiered idle lifecycle entry)
/// for the full incident and the general rootfs-is-a-path-not-a-value
/// invariant it follows from. Archiving only relocates the two files
/// that genuinely can move, `state.snap` and `mem.bin`.
///
/// `snapshot`'s own path fields are updated **immediately after each
/// individual file's move succeeds**, not all at once at the end — so if
/// this returns `Err` partway through (the mem file's move failing after
/// the state file's already moved), `snapshot` always accurately reflects
/// where each file *actually* is on disk right now, even though that's
/// now a mix of the old and new directories. This can't be made fully
/// atomic (a cross-filesystem move is never atomic at the OS level), so a
/// caller that hits this needs to treat it like `reconcile()`'s own
/// "incomplete snapshot directory" case: real, needs attention, not
/// something to silently paper over or guess at.
///
/// Prefers a `rename` (instant, same-filesystem) and falls back to
/// copy-then-remove-original only if that fails — the common case is
/// `dest_dir` on a different filesystem than the hot path entirely (the
/// whole point of archiving), so this fallback is the expected path in
/// practice, not a rare corner case.
pub(crate) fn move_snapshot_files(snapshot: &mut Snapshot, dest_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dest_dir)?;

    let new_state = state_path(dest_dir);
    move_file(&snapshot.snapshot_path, &new_state)?;
    snapshot.snapshot_path = new_state;

    let new_mem = mem_path(dest_dir);
    move_file(&snapshot.mem_file_path, &new_mem)?;
    snapshot.mem_file_path = new_mem;

    Ok(())
}

// `pub(crate)`: `snapshot_history.rs`'s own retire/restore file movement
// reuses this rather than re-implementing the same rename-with-cross-
// filesystem-fallback dance a second time.
pub(crate) fn move_file(src: &Path, dst: &Path) -> io::Result<()> {
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    // Cross-filesystem rename fails (EXDEV) -- fall back to copy+remove.
    fs::copy(src, dst)?;
    fs::remove_file(src)
}

/// Never leaves a torn file at `path` on a mid-write crash: write to a
/// sibling temp file, `fsync`, then `rename` over the real path — atomic
/// within one directory on ext4/xfs/btrfs, so a reader always sees either
/// the old or new complete contents. The temp file lives next to `path`
/// (not a shared temp dir) so the rename can't cross filesystems.
pub(crate) fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp_path = PathBuf::from(tmp_name);

    {
        let mut file = fs::File::create(&tmp_path)?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    let rename_result = fs::rename(&tmp_path, path);
    if rename_result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    rename_result?;

    // Best-effort directory fsync for power-loss durability of the rename
    // itself — not every platform supports it, and the no-torn-file
    // property above holds either way.
    if let Some(dir) = path.parent() {
        if let Ok(dir_file) = fs::File::open(dir) {
            let _ = dir_file.sync_all();
        }
    }
    Ok(())
}

/// Scans `snapshots_root()` (hot) and `archive_root` and reconstructs
/// every valid `Snapshot` on disk — the reconciliation that makes
/// snapshots durable across a restart, mirroring `DriveStore::list()`'s
/// "filesystem is the source of truth" pattern. Call once at startup,
/// before the HTTP listener binds and before any `NetworkManager::lease()`
/// can race a reconciled snapshot's tap device. `archive_root` is always
/// scanned regardless of whether `Config::archive_timeout` is currently
/// set — turning it off must not orphan snapshots already archived.
///
/// A directory missing any of its three files (crash mid-create or
/// mid-archive — see `move_snapshot_files`) is skipped with a warning,
/// left on disk for manual inspection rather than guessed-at recovery.
/// Same for a `meta.json` that fails to parse.
pub fn reconcile(network: &NetworkManager, archive_root: &Path) -> HashMap<String, Snapshot> {
    let mut snapshots = HashMap::new();
    scan_root(&snapshots_root(), network, &mut snapshots);
    scan_root(archive_root, network, &mut snapshots);
    snapshots
}

/// One root's worth of `reconcile`'s work, called once per root.
fn scan_root(root: &Path, network: &NetworkManager, snapshots: &mut HashMap<String, Snapshot>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::warn!(
                error = %e,
                dir = %root.display(),
                "failed to scan a snapshots directory on startup — treating it as having nothing to reconcile"
            );
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!(error = %e, dir = %root.display(), "failed to read a directory entry while scanning snapshots");
                continue;
            }
        };
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(id) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };

        match load_one(&dir, id, network) {
            Some(snapshot) => {
                tracing::info!(snapshot_id = %id, archived = snapshot.archived_at.is_some(), "reconciled snapshot from disk");
                snapshots.insert(id.to_string(), snapshot);
            }
            None => continue,
        }
    }
}

/// Loads and validates one snapshot directory. Returns `None` (having
/// already logged why) for anything that isn't a complete, valid
/// snapshot — an empty/unrelated directory, a partial write, or a
/// corrupt metadata file.
fn load_one(dir: &Path, id: &str, network: &NetworkManager) -> Option<Snapshot> {
    let meta_file = meta_path(dir);
    let state_file = state_path(dir);
    let mem_file = mem_path(dir);

    let meta_exists = meta_file.is_file();
    let state_exists = state_file.is_file();
    let mem_exists = mem_file.is_file();

    if !meta_exists && !state_exists && !mem_exists {
        // Not a snapshot directory at all (e.g. a leftover empty dir) --
        // nothing to warn about.
        return None;
    }
    if !(meta_exists && state_exists && mem_exists) {
        tracing::warn!(
            snapshot_id = %id,
            meta_exists,
            state_exists,
            mem_exists,
            "incomplete snapshot directory found on startup (likely a crash mid-snapshot-creation) \
             — skipping; files are left on disk for manual inspection rather than guessed at"
        );
        return None;
    }

    let bytes = match fs::read(&meta_file) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(snapshot_id = %id, error = %e, "failed to read snapshot metadata file — skipping");
            return None;
        }
    };
    let meta: SnapshotMeta = match serde_json::from_slice(&bytes) {
        Ok(meta) => meta,
        Err(e) => {
            tracing::warn!(snapshot_id = %id, error = %e, "snapshot metadata file is corrupt — skipping");
            return None;
        }
    };
    if meta.id != id {
        tracing::warn!(
            snapshot_id = %id,
            meta_id = %meta.id,
            "snapshot metadata id does not match its directory name — skipping"
        );
        return None;
    }

    let config = NetworkConfig {
        tap_device: meta.tap_device,
        guest_ip: meta.guest_ip,
        gateway_ip: meta.gateway_ip,
        guest_mac: meta.guest_mac,
    };
    let lease = network.reserve(config, meta.host_octet);

    Some(Snapshot {
        id: meta.id,
        source_sandbox_id: meta.source_sandbox_id,
        snapshot_path: state_file,
        mem_file_path: mem_file,
        rootfs_path: meta.rootfs_path,
        network: lease,
        attached_drives: meta.attached_drives,
        image_id: meta.image_id,
        tags: meta.tags,
        created_at: UNIX_EPOCH + Duration::from_secs(meta.created_at_unix),
        name: meta.name,
        // Any fork live before a restart died with the daemon -- no
        // on-disk record of it to resurrect, so always starts `None`.
        forked_into: None,
        archived_at: meta.archived_at_unix.map(|secs| UNIX_EPOCH + Duration::from_secs(secs)),
        egress: meta.egress,
        parent_snapshot_id: meta.parent_snapshot_id,
        mounts: meta.mounts,
        env: meta.env,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr as Addr;

    /// Fresh, self-cleaning temp dir — real filesystem I/O, no mocking
    /// (matches `sandkiln_vmm::drive`'s `TempStore` convention).
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(test_name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sandkiln-snapshot-test-{test_name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn test_network(taps: impl IntoIterator<Item = String>) -> NetworkManager {
        NetworkManager::new("test-br0", "10.0.0.1".parse().unwrap(), "eth-test", taps)
    }

    fn sample_meta(id: &str) -> SnapshotMeta {
        SnapshotMeta {
            id: id.to_string(),
            source_sandbox_id: "sandbox-1".to_string(),
            rootfs_path: PathBuf::from("/tmp/sandkiln-rootfs-1.ext4"),
            tap_device: "tapA".to_string(),
            guest_ip: "172.16.0.5".parse::<Addr>().unwrap(),
            gateway_ip: "172.16.0.1".parse::<Addr>().unwrap(),
            guest_mac: "AA:FC:00:00:05:05".to_string(),
            host_octet: 5,
            attached_drives: vec![AttachedDrive { drive_id: "d1".to_string(), read_only: true }],
            image_id: Some("base-image".to_string()),
            tags: HashMap::from([("env".to_string(), "test".to_string())]),
            created_at_unix: 1_700_000_000,
            name: Some("sample-snapshot".to_string()),
            archived_at_unix: None,
            egress: None,
            env: HashMap::new(),
            parent_snapshot_id: None,
            mounts: vec![],
        }
    }

    fn write_full_snapshot_dir(dir: &Path, meta: &SnapshotMeta) {
        fs::create_dir_all(dir).unwrap();
        fs::write(meta_path(dir), serde_json::to_vec(meta).unwrap()).unwrap();
        fs::write(state_path(dir), b"fake state").unwrap();
        fs::write(mem_path(dir), b"fake mem").unwrap();
    }

    #[test]
    fn write_atomically_round_trips_content_and_leaves_no_tmp_file() {
        let t = TempDir::new("atomic-write");
        let target = t.path.join("meta.json");

        write_atomically(&target, b"hello world").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"hello world");
        let mut tmp_name = target.as_os_str().to_owned();
        tmp_name.push(".tmp");
        assert!(!PathBuf::from(tmp_name).exists(), "temp file must not survive a successful write");
    }

    #[test]
    fn write_atomically_overwrites_existing_content_in_full() {
        let t = TempDir::new("atomic-overwrite");
        let target = t.path.join("meta.json");

        write_atomically(&target, b"first version, quite long").unwrap();
        write_atomically(&target, b"v2").unwrap();

        assert_eq!(fs::read(&target).unwrap(), b"v2", "no trailing bytes from the longer first write may remain");
    }

    /// `persist`/`load_one` address `snapshot_dir(&self.id)` directly (no
    /// injectable root) -- this guard cleans up the real entry even if an
    /// assertion panics.
    struct RealSnapshotDir {
        id: &'static str,
        dir: PathBuf,
    }

    impl RealSnapshotDir {
        fn new(id: &'static str) -> Self {
            let dir = snapshot_dir(id);
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self { id, dir }
        }
    }

    impl Drop for RealSnapshotDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn persist_then_load_one_round_trips_every_field() {
        let real = RealSnapshotDir::new("snap-persist-round-trip");
        fs::write(state_path(&real.dir), b"state").unwrap();
        fs::write(mem_path(&real.dir), b"mem").unwrap();

        let network = test_network(["tapA".to_string(), "tapB".to_string()]);
        let snapshot = Snapshot {
            id: real.id.to_string(),
            source_sandbox_id: "sandbox-9".to_string(),
            snapshot_path: state_path(&real.dir),
            mem_file_path: mem_path(&real.dir),
            rootfs_path: PathBuf::from("/tmp/sandkiln-rootfs-9.ext4"),
            network: network.reserve(
                NetworkConfig {
                    tap_device: "tapA".to_string(),
                    guest_ip: "172.16.0.9".parse().unwrap(),
                    gateway_ip: "172.16.0.1".parse().unwrap(),
                    guest_mac: "AA:FC:00:00:09:09".to_string(),
                },
                9,
            ),
            attached_drives: vec![
                AttachedDrive { drive_id: "d1".to_string(), read_only: false },
                AttachedDrive { drive_id: "d2".to_string(), read_only: true },
            ],
            image_id: Some("custom-image-1".to_string()),
            tags: HashMap::from([("owner".to_string(), "sumit".to_string())]),
            created_at: UNIX_EPOCH + Duration::from_secs(1_700_000_123),
            name: Some("round-trip-name".to_string()),
            forked_into: None,
            archived_at: None,
            egress: None,
            env: HashMap::from([("API_KEY".to_string(), "shh".to_string())]),
            parent_snapshot_id: Some("snap-parent-1".to_string()),
            mounts: vec![Mount {
                id: "mount-1".to_string(),
                bucket: "my-bucket".to_string(),
                endpoint: "https://s3.example.com".to_string(),
                mount_path: "/mnt/data".to_string(),
                read_only: true,
            }],
        };

        snapshot.persist(&real.dir).unwrap();

        let fresh_network = test_network(["tapA".to_string(), "tapB".to_string()]);
        let loaded = load_one(&real.dir, real.id, &fresh_network).expect("a fully-written snapshot must reconcile");

        assert_eq!(loaded.id, real.id);
        assert_eq!(loaded.source_sandbox_id, "sandbox-9");
        assert_eq!(loaded.rootfs_path, PathBuf::from("/tmp/sandkiln-rootfs-9.ext4"));
        assert_eq!(loaded.network.config.tap_device, "tapA");
        assert_eq!(loaded.network.host_octet(), 9);
        assert_eq!(
            loaded.attached_drives,
            vec![
                AttachedDrive { drive_id: "d1".to_string(), read_only: false },
                AttachedDrive { drive_id: "d2".to_string(), read_only: true },
            ]
        );
        assert_eq!(loaded.image_id, Some("custom-image-1".to_string()));
        assert_eq!(loaded.tags.get("owner"), Some(&"sumit".to_string()));
        assert_eq!(loaded.created_at.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_700_000_123);
        assert_eq!(loaded.name.as_deref(), Some("round-trip-name"));
        assert_eq!(loaded.archived_at, None);
        assert_eq!(loaded.env.get("API_KEY"), Some(&"shh".to_string()));
        assert_eq!(loaded.parent_snapshot_id.as_deref(), Some("snap-parent-1"));
        assert_eq!(
            loaded.mounts,
            vec![Mount {
                id: "mount-1".to_string(),
                bucket: "my-bucket".to_string(),
                endpoint: "https://s3.example.com".to_string(),
                mount_path: "/mnt/data".to_string(),
                read_only: true,
            }]
        );

        // The reconciled snapshot's tap must be pulled out of the fresh
        // manager's free pool — the actual double-lease-prevention
        // property under test here.
        assert!(!fresh_network.free_tap_devices().contains(&"tapA".to_string()));
    }

    #[test]
    fn load_one_defaults_name_to_none_for_metadata_written_before_naming_existed() {
        // A snapshot taken before this daemon supported naming has no
        // `name` key in its meta.json at all — `#[serde(default)]` on
        // `SnapshotMeta::name` is what keeps that a normal reconcile
        // instead of a parse failure across an upgrade.
        let t = TempDir::new("no-name-key");
        let dir = t.path.join("snap-no-name");
        fs::create_dir_all(&dir).unwrap();
        let meta_without_name = serde_json::json!({
            "id": "snap-no-name",
            "source_sandbox_id": "sandbox-1",
            "rootfs_path": "/tmp/sandkiln-rootfs-1.ext4",
            "tap_device": "tapA",
            "guest_ip": "172.16.0.5",
            "gateway_ip": "172.16.0.1",
            "guest_mac": "AA:FC:00:00:05:05",
            "host_octet": 5,
            "attached_drives": [{"drive_id": "d1", "read_only": false}],
            "tags": {},
            "created_at_unix": 1_700_000_000u64,
        });
        fs::write(meta_path(&dir), serde_json::to_vec(&meta_without_name).unwrap()).unwrap();
        fs::write(state_path(&dir), b"state").unwrap();
        fs::write(mem_path(&dir), b"mem").unwrap();

        let network = test_network(["tapA".to_string()]);
        let loaded = load_one(&dir, "snap-no-name", &network).expect("must reconcile despite the missing name key");
        assert_eq!(loaded.name, None);
    }

    #[test]
    fn load_one_defaults_parent_snapshot_id_to_none_for_metadata_written_before_lineage_existed() {
        let t = TempDir::new("no-parent-snapshot-id-key");
        let dir = t.path.join("snap-no-parent");
        fs::create_dir_all(&dir).unwrap();
        let meta_without_parent = serde_json::json!({
            "id": "snap-no-parent",
            "source_sandbox_id": "sandbox-1",
            "rootfs_path": "/tmp/sandkiln-rootfs-1.ext4",
            "tap_device": "tapA",
            "guest_ip": "172.16.0.5",
            "gateway_ip": "172.16.0.1",
            "guest_mac": "AA:FC:00:00:05:05",
            "host_octet": 5,
            "attached_drives": [{"drive_id": "d1", "read_only": false}],
            "tags": {},
            "created_at_unix": 1_700_000_000u64,
        });
        fs::write(meta_path(&dir), serde_json::to_vec(&meta_without_parent).unwrap()).unwrap();
        fs::write(state_path(&dir), b"state").unwrap();
        fs::write(mem_path(&dir), b"mem").unwrap();

        let network = test_network(["tapA".to_string()]);
        let loaded = load_one(&dir, "snap-no-parent", &network).expect("must reconcile despite the missing parent_snapshot_id key");
        assert_eq!(loaded.parent_snapshot_id, None);
    }

    #[test]
    fn load_one_defaults_mounts_to_empty_for_metadata_written_before_remote_mounts_existed() {
        let t = TempDir::new("no-mounts-key");
        let dir = t.path.join("snap-no-mounts");
        fs::create_dir_all(&dir).unwrap();
        let meta_without_mounts = serde_json::json!({
            "id": "snap-no-mounts",
            "source_sandbox_id": "sandbox-1",
            "rootfs_path": "/tmp/sandkiln-rootfs-1.ext4",
            "tap_device": "tapA",
            "guest_ip": "172.16.0.5",
            "gateway_ip": "172.16.0.1",
            "guest_mac": "AA:FC:00:00:05:05",
            "host_octet": 5,
            "attached_drives": [{"drive_id": "d1", "read_only": false}],
            "tags": {},
            "created_at_unix": 1_700_000_000u64,
        });
        fs::write(meta_path(&dir), serde_json::to_vec(&meta_without_mounts).unwrap()).unwrap();
        fs::write(state_path(&dir), b"state").unwrap();
        fs::write(mem_path(&dir), b"mem").unwrap();

        let network = test_network(["tapA".to_string()]);
        let loaded = load_one(&dir, "snap-no-mounts", &network).expect("must reconcile despite the missing mounts key");
        assert_eq!(loaded.mounts, Vec::new());
    }

    #[test]
    fn load_one_skips_a_directory_missing_state_snap() {
        let t = TempDir::new("partial-missing-state");
        let dir = t.path.join("snap-partial");
        fs::create_dir_all(&dir).unwrap();
        fs::write(meta_path(&dir), serde_json::to_vec(&sample_meta("snap-partial")).unwrap()).unwrap();
        fs::write(mem_path(&dir), b"mem only").unwrap();
        // state.snap deliberately absent — simulates a crash between
        // `Vm::snapshot` writing mem.bin and finishing state.snap.

        let network = test_network(["tapA".to_string()]);
        assert!(load_one(&dir, "snap-partial", &network).is_none());
    }

    #[test]
    fn load_one_skips_a_directory_missing_meta_json() {
        let t = TempDir::new("partial-missing-meta");
        let dir = t.path.join("snap-partial2");
        fs::create_dir_all(&dir).unwrap();
        fs::write(state_path(&dir), b"state").unwrap();
        fs::write(mem_path(&dir), b"mem").unwrap();
        // meta.json deliberately absent — simulates a crash before the
        // metadata-persist step ran at all.

        let network = test_network(["tapA".to_string()]);
        assert!(load_one(&dir, "snap-partial2", &network).is_none());
    }

    #[test]
    fn load_one_skips_a_directory_with_corrupt_metadata() {
        let t = TempDir::new("corrupt-meta");
        let dir = t.path.join("snap-corrupt");
        write_full_snapshot_dir(&dir, &sample_meta("snap-corrupt"));
        fs::write(meta_path(&dir), b"not valid json{{{").unwrap();

        let network = test_network(["tapA".to_string()]);
        assert!(load_one(&dir, "snap-corrupt", &network).is_none());
    }

    #[test]
    fn load_one_ignores_an_empty_unrelated_directory_without_warning_fields() {
        let t = TempDir::new("empty-dir");
        let dir = t.path.join("not-a-snapshot");
        fs::create_dir_all(&dir).unwrap();

        let network = test_network(["tapA".to_string()]);
        assert!(load_one(&dir, "not-a-snapshot", &network).is_none());
    }

    #[test]
    fn load_one_returns_none_when_metadata_id_does_not_match_directory_name() {
        let t = TempDir::new("id-mismatch");
        let dir = t.path.join("dir-name");
        write_full_snapshot_dir(&dir, &sample_meta("different-id"));

        let network = test_network(["tapA".to_string()]);
        assert!(load_one(&dir, "dir-name", &network).is_none());
    }

    /// A snapshot written by a daemon build from before `image_id` existed
    /// has no such field in its `meta.json` at all — `#[serde(default)]`
    /// on `SnapshotMeta::image_id` is what keeps that file reconcilable
    /// after an upgrade instead of getting skipped as "corrupt metadata".
    #[test]
    fn load_one_treats_metadata_with_no_image_id_field_as_the_pre_image_default_rootfs() {
        let t = TempDir::new("pre-image-field-meta");
        let dir = t.path.join("snap-legacy");
        fs::create_dir_all(&dir).unwrap();

        let legacy_json = serde_json::json!({
            "id": "snap-legacy",
            "source_sandbox_id": "sandbox-1",
            "rootfs_path": "/tmp/sandkiln-rootfs-1.ext4",
            "tap_device": "tapA",
            "guest_ip": "172.16.0.5",
            "gateway_ip": "172.16.0.1",
            "guest_mac": "AA:FC:00:00:05:05",
            "host_octet": 5,
            "attached_drives": [{"drive_id": "d1", "read_only": false}],
            "tags": {"env": "test"},
            "created_at_unix": 1_700_000_000_u64,
        });
        fs::write(meta_path(&dir), serde_json::to_vec(&legacy_json).unwrap()).unwrap();
        fs::write(state_path(&dir), b"state").unwrap();
        fs::write(mem_path(&dir), b"mem").unwrap();

        let network = test_network(["tapA".to_string()]);
        let loaded = load_one(&dir, "snap-legacy", &network).expect("pre-image_id metadata must still reconcile");
        assert_eq!(loaded.image_id, None);
    }

    #[test]
    fn load_one_reserves_the_snapshots_tap_and_host_octet_out_of_the_pool() {
        let t = TempDir::new("reserves-pool");
        let dir = t.path.join("snap-reserve");
        write_full_snapshot_dir(&dir, &sample_meta("snap-reserve"));

        let network = test_network(["tapA".to_string(), "tapB".to_string()]);
        let loaded = load_one(&dir, "snap-reserve", &network).expect("valid snapshot must reconcile");
        assert_eq!(loaded.network.config.tap_device, "tapA");

        assert!(
            !network.free_tap_devices().contains(&"tapA".to_string()),
            "reconciling a snapshot must remove its held tap from the live pool so a later \
             live lease() cannot double-hand it to a different sandbox"
        );
        assert!(network.free_tap_devices().contains(&"tapB".to_string()));
    }

    #[test]
    fn move_file_relocates_real_content_within_one_filesystem() {
        let t = TempDir::new("move-file-same-fs");
        let src = t.path.join("src.bin");
        let dst = t.path.join("nested").join("dst.bin");
        fs::create_dir_all(dst.parent().unwrap()).unwrap();
        fs::write(&src, b"snapshot bytes").unwrap();

        move_file(&src, &dst).unwrap();

        assert_eq!(fs::read(&dst).unwrap(), b"snapshot bytes");
        assert!(!src.exists(), "the source must be gone after a move, not just copied");
    }

    #[test]
    fn move_file_fails_cleanly_for_a_missing_source() {
        let t = TempDir::new("move-file-missing");
        let result = move_file(&t.path.join("does-not-exist.bin"), &t.path.join("dst.bin"));
        assert!(result.is_err());
    }

    #[test]
    fn move_snapshot_files_relocates_state_and_mem_but_leaves_rootfs_exactly_where_it_was() {
        let t = TempDir::new("move-snapshot-files");
        let hot = t.path.join("hot");
        let archive = t.path.join("archive");
        fs::create_dir_all(&hot).unwrap();
        let state_file = hot.join("state.snap");
        let mem_file = hot.join("mem.bin");
        let rootfs_file = t.path.join("sandkiln-rootfs-loose.ext4");
        fs::write(&state_file, b"state").unwrap();
        fs::write(&mem_file, b"mem").unwrap();
        fs::write(&rootfs_file, b"rootfs").unwrap();

        let network = test_network(["tapA".to_string()]);
        let mut snapshot = Snapshot {
            id: "snap-move".to_string(),
            source_sandbox_id: "sandbox-1".to_string(),
            snapshot_path: state_file.clone(),
            mem_file_path: mem_file.clone(),
            rootfs_path: rootfs_file.clone(),
            network: network.reserve(
                NetworkConfig {
                    tap_device: "tapA".to_string(),
                    guest_ip: "172.16.0.9".parse().unwrap(),
                    gateway_ip: "172.16.0.1".parse().unwrap(),
                    guest_mac: "AA:FC:00:00:09:09".to_string(),
                },
                9,
            ),
            attached_drives: vec![],
            image_id: None,
            tags: HashMap::new(),
            created_at: SystemTime::now(),
            name: None,
            forked_into: None,
            archived_at: None,
            egress: None,
            env: HashMap::new(),
            parent_snapshot_id: None,
            mounts: vec![],
        };

        move_snapshot_files(&mut snapshot, &archive).unwrap();

        assert_eq!(snapshot.snapshot_path, state_path(&archive));
        assert_eq!(snapshot.mem_file_path, mem_path(&archive));
        assert_eq!(fs::read(&snapshot.snapshot_path).unwrap(), b"state");
        assert_eq!(fs::read(&snapshot.mem_file_path).unwrap(), b"mem");
        assert!(!state_file.exists());
        assert!(!mem_file.exists());

        // The one thing this whole test exists to pin down: Firecracker
        // bakes the rootfs backing file's absolute host path into
        // state.snap itself, with no override at resume time -- moving
        // it would silently break every future resume/fork of this
        // snapshot. `rootfs_path` must be untouched, and the file itself
        // must still be exactly where it started.
        assert_eq!(snapshot.rootfs_path, rootfs_file);
        assert_eq!(fs::read(&snapshot.rootfs_path).unwrap(), b"rootfs");
        assert!(rootfs_file.exists());
    }
}

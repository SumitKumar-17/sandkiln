use crate::state::AttachedDrive;
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

/// A saved, stopped microVM: memory and device state on disk, ready to be
/// resumed into a new live sandbox. This is what a sandbox becomes
/// instead of being fully torn down — no `Vm`, no running process, but
/// not gone either.
pub struct Snapshot {
    pub id: String,
    /// The id of the sandbox this was taken from. Purely informational —
    /// that sandbox no longer exists by the time a `Snapshot` exists.
    pub source_sandbox_id: String,
    /// The state file written by `Vm::snapshot`.
    pub snapshot_path: PathBuf,
    /// The guest-memory file written by `Vm::snapshot`.
    pub mem_file_path: PathBuf,
    /// The rootfs image carried over unmodified from the sandbox that
    /// made this snapshot. Firecracker's snapshot only records this
    /// file's host path, not its contents, so it has to stay right where
    /// it was when the snapshot was taken — ownership passes to the new
    /// sandbox on resume, or the file is removed if this snapshot is
    /// deleted outright instead.
    pub rootfs_path: PathBuf,
    /// Held rather than released back to `NetworkManager`'s pool: the
    /// guest's network config (IP, MAC) was finalized via kernel boot
    /// args at the sandbox's original boot and is frozen into the
    /// snapshotted memory image, so whatever resumes this snapshot must
    /// reattach the exact same tap device rather than get a fresh lease.
    /// See `sandkiln_vmm::vm::Vm::resume`'s doc comment for why.
    pub network: Lease,
    /// Carried over from the source sandbox's `attached_drives`, read-only
    /// flag included — a drive's data lives inside the snapshotted
    /// memory/rootfs state the same way the network config does, so it
    /// must stay marked attached while this snapshot exists, with the
    /// same read-only flag it was attached with, or a caller could attach
    /// a read-write copy of a drive this snapshot still holds read-write
    /// and corrupt it via two VMs writing to one file.
    pub attached_drives: Vec<AttachedDrive>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// Carried over from the source sandbox's `Sandbox::name`, if it had
    /// one — see that field's doc comment. Lets a caller find this
    /// snapshot again by the same name it created the sandbox with,
    /// whether via `GET /sandboxes/by-name/:name` (once resumed) or
    /// `POST /sandboxes/get-or-create`.
    pub name: Option<String>,
    /// Id of the live sandbox currently forked from this snapshot without
    /// consuming it (`POST /snapshots/:id/fork`), if any. `Vm::resume`'s
    /// `/snapshot/load` reopens the exact rootfs file this snapshot
    /// records, and — if the source sandbox was networked — the exact tap
    /// device, since the guest's IP/MAC were finalized at the source
    /// sandbox's *original* boot and are frozen into the snapshotted
    /// memory image (see `sandkiln_vmm::vm::Vm::resume`'s doc comment).
    /// A second live descendant sharing either at once means two
    /// Firecracker processes writing one rootfs file, or two guests
    /// presenting the same boot-time IP/MAC on the bridge simultaneously —
    /// real corruption and a real network collision, not hypothetical
    /// ones. This field is the lock that rules both out: set while a fork
    /// is being resumed and cleared only once that descendant's `Vm` has
    /// actually been killed (`routes_sandbox::stop_sandbox_by_id`), and
    /// checked by `fork_snapshot`, `resume_snapshot`, `delete_snapshot`,
    /// and `snapshot_sandbox` alike before any of them touch this
    /// snapshot's shared resources.
    pub forked_into: Option<String>,
}

/// On-disk mirror of everything about a `Snapshot` that isn't already
/// implied by its directory's contents (`state.snap`/`mem.bin` live at
/// fixed names under `snapshot_dir(id)`, so they aren't duplicated here).
/// Written by `Snapshot::persist`, read back by `reconcile` at daemon
/// startup — this file existing alongside `state.snap`/`mem.bin` is what
/// makes a `Snapshot` durable across a restart, matching the same
/// "filesystem is the source of truth" convention `sandkiln_vmm::drive`
/// already uses for drives.
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
    tags: HashMap<String, String>,
    created_at_unix: u64,
    /// `#[serde(default)]` so a snapshot written to disk before naming
    /// existed still reconciles cleanly on a daemon upgrade — its
    /// `meta.json` simply has no `name` key, and that must deserialize as
    /// `None`, not fail `reconcile()` outright.
    #[serde(default)]
    name: Option<String>,
}

impl Snapshot {
    /// Writes this snapshot's metadata to `snapshot_dir(&self.id)`,
    /// atomically (write-then-rename, see `write_atomically`) so a crash
    /// mid-write can never leave a torn, half-written metadata file
    /// behind for `reconcile` to trip over. Assumes the directory already
    /// exists — `snapshot_sandbox` creates it before this is ever called.
    pub fn persist(&self) -> io::Result<()> {
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
            tags: self.tags.clone(),
            created_at_unix: self.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            name: self.name.clone(),
        };
        let json = serde_json::to_vec_pretty(&meta).map_err(|e| io::Error::other(format!("serializing snapshot metadata: {e}")))?;
        write_atomically(&meta_path(&snapshot_dir(&self.id)), &json)
    }
}

/// Where every snapshot's per-snapshot directory lives — a dedicated
/// directory per snapshot under the daemon's temp dir, alongside the
/// loose `sandkiln-rootfs-*.ext4` files `create_sandbox` writes there —
/// mirrors that same "OS temp dir, daemon-prefixed" convention rather
/// than inventing a new storage location.
pub fn snapshots_root() -> PathBuf {
    std::env::temp_dir().join("sandkiln-snapshots")
}

/// Where one snapshot's state, memory, and metadata files live.
pub fn snapshot_dir(snapshot_id: &str) -> PathBuf {
    snapshots_root().join(snapshot_id)
}

fn meta_path(dir: &Path) -> PathBuf {
    dir.join("meta.json")
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join("state.snap")
}

fn mem_path(dir: &Path) -> PathBuf {
    dir.join("mem.bin")
}

/// Writes `contents` to `path` without ever leaving a torn (partially
/// written) file at `path` if the process crashes mid-write: write to a
/// sibling temp file, `fsync` it, then `rename` over the real path.
/// `rename` within one directory is atomic on every filesystem this
/// project targets (ext4, xfs, btrfs), so a reader of `path` always sees
/// either the previous complete contents or the new complete contents,
/// never a mix. The temp file lives next to `path` (not in a shared temp
/// dir) specifically so the rename is guaranteed to stay on one
/// filesystem — a cross-filesystem rename is not atomic.
fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
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

    // Best-effort: durability of the rename itself surviving a real power
    // loss needs the directory entry fsynced too, but not every
    // filesystem/platform this runs on supports opening a directory for
    // that, and the write-then-rename already delivers the property this
    // is actually used for — no reader ever observes a torn file, even if
    // the very last fsync below is skipped.
    if let Some(dir) = path.parent() {
        if let Ok(dir_file) = fs::File::open(dir) {
            let _ = dir_file.sync_all();
        }
    }
    Ok(())
}

/// Scans `snapshots_root()` and reconstructs every valid `Snapshot`
/// found on disk — the reconciliation step that makes snapshots durable
/// across a daemon restart, mirroring `DriveStore::list()`'s "the
/// filesystem is the source of truth" pattern. Call once at startup,
/// before the HTTP listener starts accepting connections and before any
/// live `NetworkManager::lease()` call can race a reconciled snapshot's
/// held tap device (see `NetworkManager::reserve`).
///
/// A snapshot directory missing any of its three files (`meta.json`,
/// `state.snap`, `mem.bin`) — the signature of a crash mid-snapshot-
/// creation, since all three are only ever produced together by
/// `snapshot_sandbox` — is treated as invalid and skipped with a warning
/// log rather than reconciled or silently deleted; the files are left in
/// place for manual inspection rather than the daemon guessing at
/// recovery. Likewise a `meta.json` that fails to parse.
pub fn reconcile(network: &NetworkManager) -> HashMap<String, Snapshot> {
    let mut snapshots = HashMap::new();
    let root = snapshots_root();

    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return snapshots,
        Err(e) => {
            tracing::warn!(
                error = %e,
                dir = %root.display(),
                "failed to scan snapshots directory on startup — starting with no reconciled snapshots"
            );
            return snapshots;
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
                tracing::info!(snapshot_id = %id, "reconciled snapshot from disk");
                snapshots.insert(id.to_string(), snapshot);
            }
            None => continue,
        }
    }

    snapshots
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
        // Not a snapshot directory at all (e.g. leftover empty dir from a
        // resume that already cleaned up everything but the directory
        // itself) — nothing to warn about.
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
        tags: meta.tags,
        created_at: UNIX_EPOCH + Duration::from_secs(meta.created_at_unix),
        name: meta.name,
        // Any fork that was live before a restart died with the daemon
        // along with every other live sandbox — there's no on-disk record
        // of a fork to resurrect (see `routes_snapshot`'s module doc
        // comment: only the original snapshot's files persist), so a
        // reconciled snapshot always starts with no live fork.
        forked_into: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr as Addr;

    /// A fresh, self-cleaning temp directory standing in for
    /// `snapshots_root()` — real filesystem I/O, no mocking, matching
    /// `sandkiln_vmm::drive`'s `TempStore` test convention. `reconcile`
    /// itself always reads `snapshots_root()`, so these tests exercise
    /// the same directory-scan/load logic directly via `load_one`
    /// (unit-level) and via a temporarily-redirected root for the
    /// integration-shaped tests (`with_root`).
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
            tags: HashMap::from([("env".to_string(), "test".to_string())]),
            created_at_unix: 1_700_000_000,
            name: Some("sample-snapshot".to_string()),
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

    /// `persist`/`load_one` address `snapshot_dir(&self.id)` directly
    /// (there's no injectable root, mirroring `snapshot_dir`'s existing
    /// hardcoded-location convention), so this guard cleans up the real
    /// `snapshots_root()` entry even if an assertion panics partway
    /// through.
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
            tags: HashMap::from([("owner".to_string(), "sumit".to_string())]),
            created_at: UNIX_EPOCH + Duration::from_secs(1_700_000_123),
            name: Some("round-trip-name".to_string()),
            forked_into: None,
        };

        snapshot.persist().unwrap();

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
        assert_eq!(loaded.tags.get("owner"), Some(&"sumit".to_string()));
        assert_eq!(loaded.created_at.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_700_000_123);
        assert_eq!(loaded.name.as_deref(), Some("round-trip-name"));

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
}

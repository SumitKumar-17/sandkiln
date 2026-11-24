//! Startup reconciliation of held snapshots from disk.
//!
//! The filesystem is the source of truth (same pattern as `DriveStore::list`):
//! `reconcile` scans the hot and archive roots, `load_one` validates one
//! directory and re-reserves its network lease. A directory missing any of its
//! three files, or with unreadable metadata, is skipped with a warning and left
//! on disk for manual inspection.

use super::{Snapshot, SnapshotMeta};
use crate::snapshots::layout::{mem_path, meta_path, snapshots_root, state_path};
use sandkiln_vmm::network::NetworkManager;
use sandkiln_vmm::vm::NetworkConfig;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

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
    use crate::snapshots::layout::snapshot_dir;
    use crate::snapshots::test_support::{test_network, TempDir};
    use crate::state::{AttachedDrive, Mount};
    use std::net::Ipv4Addr as Addr;
    use std::path::PathBuf;

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
}

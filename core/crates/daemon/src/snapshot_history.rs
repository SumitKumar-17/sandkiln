//! Retired snapshot checkpoints — the durable record behind "time-travel
//! restore" (see `ROADMAP.md`'s "Persistence and snapshotting" section).
//!
//! Today, `routes_snapshot::resume_snapshot_by_id` deletes the snapshot it
//! resumes: `state.snap`/`mem.bin` are removed, and the new live sandbox
//! takes over the *exact same* rootfs file the snapshot pointed at. That
//! makes "restore to an earlier point" structurally impossible once you've
//! ever moved forward — the earlier point's files are gone, and even if
//! they weren't, the rootfs they'd need is now being mutated by whatever
//! came after.
//!
//! This module is the fix: `resume_snapshot_by_id` (by default — see its
//! own doc comment for the `retain_history` opt-out) now *retires* the
//! snapshot it resumes instead of deleting it. Retiring means:
//! - `state.snap`/`mem.bin` move into this module's own directory tree
//!   (`retired_root()`), under the same three-file-plus-`meta.json` shape
//!   `snapshot.rs` already uses.
//! - The rootfs file it referenced is left **completely untouched** —
//!   ownership doesn't transfer. Instead, the *new* live sandbox gets a
//!   fresh `routes_sandbox::clone_rootfs` copy of it, so any future write
//!   from continued use can never retroactively corrupt a checkpoint
//!   whose `state.snap` still describes the file's contents as of the
//!   moment it was taken. This is the load-bearing property of the whole
//!   feature: without it, restoring an "earlier" checkpoint could hand
//!   back a memory image that thinks a file contains bytes the (shared,
//!   since-mutated) rootfs no longer has.
//! - The retired record itself is inert data, not a live resource: unlike
//!   `Snapshot`, it holds a `NetworkConfig` (plus the `host_octet` needed
//!   to re-derive a `Lease`), **not** a live `sandkiln_vmm::network::Lease`
//!   — nothing is reserved out of `NetworkManager`'s free pool just by a
//!   checkpoint sitting in history. A lease is only ever reserved at
//!   *restore* time (`routes_snapshot_history::restore_snapshot_history`),
//!   exactly like a startup `reconcile()` reserves one for an on-disk
//!   `Snapshot` — see that function for why this still needs an explicit
//!   conflict check first (`AppState::tap_device_holder`): unlike
//!   `reconcile()`'s once-at-startup-before-anything-else-runs guarantee,
//!   a restore can race a live sandbox or held snapshot that's *already*
//!   using this exact tap device (the same frozen network identity every
//!   checkpoint in one lineage shares — see `sandkiln_vmm::vm::Vm::resume`'s
//!   doc comment for why that identity can never change post-hoc).
//!
//! **Sequential, not branching**: restoring an old checkpoint doesn't
//! delete or invalidate whatever came after it in that lineage — they stay
//! in history too, exactly like an old git commit's descendants survive a
//! `git checkout` of an ancestor. What restoring *does* require is that
//! nothing else sharing this checkpoint's network identity is currently
//! live or held right now (`tap_device_holder`'s job) — the same
//! one-live-descendant-at-a-time rule `Snapshot::forked_into` already
//! enforces for fork, generalized across a checkpoint's entire history
//! instead of just its single most recent snapshot. True *parallel*
//! branching (two checkpoints from one lineage live at once) is a
//! different, harder problem — see `routes_snapshot.rs`'s own module doc
//! comment on why concurrent forking is genuinely open (the guest's own
//! network identity can't be changed without in-guest cooperation this
//! project's guest agent doesn't have) — restoring never attempts it.
//!
//! Restoring a checkpoint does **not** consume it — same `clone_rootfs`
//! trick in reverse: the *new* sandbox produced by a restore gets its own
//! fresh rootfs clone, so the retired checkpoint's own files stay exactly
//! as they were and can be restored again later, as many times as wanted.

use crate::state::AttachedDrive;
use sandkiln_vmm::egress::EgressPolicy;
use sandkiln_vmm::vm::NetworkConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// One retired checkpoint — everything needed to restore it later, minus
/// anything that requires an actively-held daemon resource (a live
/// `Lease`). See the module doc comment for the full design. `Clone`
/// (unlike `Snapshot`, which deliberately isn't — it holds a `Lease`
/// nothing should ever silently duplicate) is safe and needed here
/// precisely because nothing in this struct is an exclusively-held
/// resource: `routes_snapshot_history::restore_snapshot_history_by_id`
/// clones one out from under a short-lived lock rather than removing it
/// from `AppState::retired_snapshots`, since restoring doesn't consume it.
#[derive(Clone)]
pub struct RetiredSnapshot {
    pub id: String,
    pub source_sandbox_id: String,
    pub snapshot_path: PathBuf,
    pub mem_file_path: PathBuf,
    /// Left exactly where it was at the moment of retirement — never
    /// moved, never mutated. See the module doc comment for why this is
    /// the property that makes retiring safe at all.
    pub rootfs_path: PathBuf,
    /// Not a live `Lease` — see the module doc comment. `host_octet` is
    /// carried alongside since `NetworkManager::reserve` needs it (mirrors
    /// exactly what `SnapshotMeta` already stores for the same reason).
    pub network: NetworkConfig,
    pub host_octet: u8,
    pub attached_drives: Vec<AttachedDrive>,
    pub image_id: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub retired_at: SystemTime,
    pub name: Option<String>,
    /// The snapshot this checkpoint's own source sandbox was resumed or
    /// forked from, if any — same lineage-parent-pointer meaning as
    /// `Snapshot::parent_snapshot_id`, carried straight over so a
    /// checkpoint's history doesn't go dark just because it was later
    /// retired instead of staying "hot."
    pub parent_snapshot_id: Option<String>,
    pub egress: Option<EgressPolicy>,
}

/// On-disk mirror of `RetiredSnapshot`, written by `persist`, read back by
/// `reconcile` — same "filesystem is the source of truth" shape
/// `snapshot.rs`'s own `SnapshotMeta` uses, deliberately not shared as one
/// type with it: a `Snapshot`'s metadata round-trips a live `Lease`
/// (`NetworkManager::reserve` needs to reclaim it at startup), a retired
/// checkpoint's metadata deliberately does not reclaim anything until an
/// explicit restore — conflating the two would blur a real distinction
/// (hot, resource-holding vs. dormant, inert) into one struct with a flag.
#[derive(Serialize, Deserialize)]
struct RetiredSnapshotMeta {
    id: String,
    source_sandbox_id: String,
    rootfs_path: PathBuf,
    tap_device: String,
    guest_ip: Ipv4Addr,
    gateway_ip: Ipv4Addr,
    guest_mac: String,
    host_octet: u8,
    attached_drives: Vec<AttachedDrive>,
    image_id: Option<String>,
    tags: HashMap<String, String>,
    created_at_unix: u64,
    retired_at_unix: u64,
    name: Option<String>,
    parent_snapshot_id: Option<String>,
    egress: Option<EgressPolicy>,
}

impl RetiredSnapshot {
    /// Mirrors `Snapshot::persist` exactly, minus anything `Lease`-shaped
    /// — see this module's own doc comment for why a retired checkpoint
    /// carries a bare `NetworkConfig`/`host_octet` instead.
    pub fn persist(&self, dir: &Path) -> io::Result<()> {
        let meta = RetiredSnapshotMeta {
            id: self.id.clone(),
            source_sandbox_id: self.source_sandbox_id.clone(),
            rootfs_path: self.rootfs_path.clone(),
            tap_device: self.network.tap_device.clone(),
            guest_ip: self.network.guest_ip,
            gateway_ip: self.network.gateway_ip,
            guest_mac: self.network.guest_mac.clone(),
            host_octet: self.host_octet,
            attached_drives: self.attached_drives.clone(),
            image_id: self.image_id.clone(),
            tags: self.tags.clone(),
            created_at_unix: self.created_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            retired_at_unix: self.retired_at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs(),
            name: self.name.clone(),
            parent_snapshot_id: self.parent_snapshot_id.clone(),
            egress: self.egress.clone(),
        };
        let json =
            serde_json::to_vec_pretty(&meta).map_err(|e| io::Error::other(format!("serializing retired-snapshot metadata: {e}")))?;
        crate::snapshot::write_atomically(&crate::snapshot::meta_path(dir), &json)
    }
}

/// Where every retired checkpoint's own per-id directory lives — a
/// sibling of `snapshot::snapshots_root()` under the same daemon temp
/// dir, same "OS temp dir, daemon-prefixed" convention, deliberately a
/// separate root rather than a subdirectory of `snapshots_root()` itself:
/// `snapshot::reconcile` scans its root expecting every entry to be a
/// *hot, lease-holding* `Snapshot` — mixing in lease-free retired
/// checkpoints there would either break that assumption or need every
/// caller of `snapshots_root()` to start filtering, for no real benefit.
pub fn retired_root() -> PathBuf {
    std::env::temp_dir().join("sandkiln-snapshot-history")
}

/// Where one retired checkpoint's state, memory, and metadata files live.
pub fn retired_dir(id: &str) -> PathBuf {
    retired_root().join(id)
}

/// Scans `retired_root()` and reconstructs every valid `RetiredSnapshot`
/// found on disk — the reconciliation step that makes retired checkpoints
/// durable across a daemon restart, mirroring `snapshot::reconcile`'s own
/// shape closely. **Deliberately does not touch `NetworkManager` at all**
/// (no `reserve()` call) — see the module doc comment: a retired
/// checkpoint doesn't hold a live lease, so there's nothing to reclaim out
/// of the free pool just because it exists on disk. Call at startup
/// alongside `snapshot::reconcile`, order doesn't matter between the two
/// (neither touches the other's resources).
pub fn reconcile() -> HashMap<String, RetiredSnapshot> {
    let mut retired = HashMap::new();
    let entries = match fs::read_dir(retired_root()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return retired,
        Err(e) => {
            tracing::warn!(error = %e, dir = %retired_root().display(), "failed to scan the snapshot-history directory on startup — treating it as having nothing to reconcile");
            return retired;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!(error = %e, dir = %retired_root().display(), "failed to read a directory entry while scanning snapshot history");
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

        if let Some(checkpoint) = load_one(&dir, id) {
            tracing::info!(retired_snapshot_id = %id, "reconciled a retired snapshot checkpoint from disk");
            retired.insert(id.to_string(), checkpoint);
        }
    }
    retired
}

/// Loads and validates one retired-checkpoint directory. `None` (having
/// already logged why) for anything that isn't a complete, valid one —
/// same three-cases-of-invalid shape as `snapshot::load_one`.
fn load_one(dir: &Path, id: &str) -> Option<RetiredSnapshot> {
    let meta_file = crate::snapshot::meta_path(dir);
    let state_file = crate::snapshot::state_path(dir);
    let mem_file = crate::snapshot::mem_path(dir);

    let meta_exists = meta_file.is_file();
    let state_exists = state_file.is_file();
    let mem_exists = mem_file.is_file();

    if !meta_exists && !state_exists && !mem_exists {
        return None;
    }
    if !(meta_exists && state_exists && mem_exists) {
        tracing::warn!(
            retired_snapshot_id = %id,
            meta_exists,
            state_exists,
            mem_exists,
            "incomplete retired-snapshot directory found on startup (likely a crash mid-retire) \
             — skipping; files are left on disk for manual inspection rather than guessed at"
        );
        return None;
    }

    let bytes = match fs::read(&meta_file) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(retired_snapshot_id = %id, error = %e, "failed to read retired-snapshot metadata file — skipping");
            return None;
        }
    };
    let meta: RetiredSnapshotMeta = match serde_json::from_slice(&bytes) {
        Ok(meta) => meta,
        Err(e) => {
            tracing::warn!(retired_snapshot_id = %id, error = %e, "retired-snapshot metadata file is corrupt — skipping");
            return None;
        }
    };
    if meta.id != id {
        tracing::warn!(
            retired_snapshot_id = %id,
            meta_id = %meta.id,
            "retired-snapshot metadata id does not match its directory name — skipping"
        );
        return None;
    }

    Some(RetiredSnapshot {
        id: meta.id,
        source_sandbox_id: meta.source_sandbox_id,
        snapshot_path: state_file,
        mem_file_path: mem_file,
        rootfs_path: meta.rootfs_path,
        network: NetworkConfig {
            tap_device: meta.tap_device,
            guest_ip: meta.guest_ip,
            gateway_ip: meta.gateway_ip,
            guest_mac: meta.guest_mac,
        },
        host_octet: meta.host_octet,
        attached_drives: meta.attached_drives,
        image_id: meta.image_id,
        tags: meta.tags,
        created_at: UNIX_EPOCH + Duration::from_secs(meta.created_at_unix),
        retired_at: UNIX_EPOCH + Duration::from_secs(meta.retired_at_unix),
        name: meta.name,
        parent_snapshot_id: meta.parent_snapshot_id,
        egress: meta.egress,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(test_name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sandkiln-retired-test-{test_name}-{}", std::process::id()));
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

    fn sample_checkpoint(id: &str, dir: &Path) -> RetiredSnapshot {
        RetiredSnapshot {
            id: id.to_string(),
            source_sandbox_id: "sandbox-1".to_string(),
            snapshot_path: crate::snapshot::state_path(dir),
            mem_file_path: crate::snapshot::mem_path(dir),
            rootfs_path: PathBuf::from("/tmp/sandkiln-rootfs-1.ext4"),
            network: NetworkConfig {
                tap_device: "tapA".to_string(),
                guest_ip: "172.16.0.5".parse().unwrap(),
                gateway_ip: "172.16.0.1".parse().unwrap(),
                guest_mac: "AA:FC:00:00:05:05".to_string(),
            },
            host_octet: 5,
            attached_drives: vec![AttachedDrive { drive_id: "d1".to_string(), read_only: true }],
            image_id: Some("base-image".to_string()),
            tags: HashMap::from([("env".to_string(), "test".to_string())]),
            created_at: UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            retired_at: UNIX_EPOCH + Duration::from_secs(1_700_000_500),
            name: Some("sample-checkpoint".to_string()),
            parent_snapshot_id: Some("snap-parent-1".to_string()),
            egress: None,
        }
    }

    #[test]
    fn persist_then_load_one_round_trips_every_field() {
        let t = TempDir::new("round-trip");
        fs::write(crate::snapshot::state_path(&t.path), b"state").unwrap();
        fs::write(crate::snapshot::mem_path(&t.path), b"mem").unwrap();

        let checkpoint = sample_checkpoint("retired-1", &t.path);
        checkpoint.persist(&t.path).unwrap();

        let loaded = load_one(&t.path, "retired-1").expect("a fully-written retired checkpoint must reconcile");
        assert_eq!(loaded.id, "retired-1");
        assert_eq!(loaded.source_sandbox_id, "sandbox-1");
        assert_eq!(loaded.rootfs_path, PathBuf::from("/tmp/sandkiln-rootfs-1.ext4"));
        assert_eq!(loaded.network.tap_device, "tapA");
        assert_eq!(loaded.host_octet, 5);
        assert_eq!(loaded.attached_drives, vec![AttachedDrive { drive_id: "d1".to_string(), read_only: true }]);
        assert_eq!(loaded.image_id, Some("base-image".to_string()));
        assert_eq!(loaded.tags.get("env"), Some(&"test".to_string()));
        assert_eq!(loaded.created_at.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_700_000_000);
        assert_eq!(loaded.retired_at.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_700_000_500);
        assert_eq!(loaded.name.as_deref(), Some("sample-checkpoint"));
        assert_eq!(loaded.parent_snapshot_id.as_deref(), Some("snap-parent-1"));
    }

    #[test]
    fn load_one_skips_a_directory_missing_state_snap() {
        let t = TempDir::new("missing-state");
        fs::write(crate::snapshot::mem_path(&t.path), b"mem").unwrap();
        sample_checkpoint("retired-2", &t.path).persist(&t.path).unwrap();
        // state.snap deliberately never written.

        assert!(load_one(&t.path, "retired-2").is_none());
    }

    #[test]
    fn load_one_skips_a_directory_missing_meta_json() {
        let t = TempDir::new("missing-meta");
        fs::write(crate::snapshot::state_path(&t.path), b"state").unwrap();
        fs::write(crate::snapshot::mem_path(&t.path), b"mem").unwrap();
        // meta.json deliberately never written.

        assert!(load_one(&t.path, "retired-3").is_none());
    }

    #[test]
    fn load_one_skips_a_directory_with_corrupt_metadata() {
        let t = TempDir::new("corrupt-meta");
        fs::write(crate::snapshot::state_path(&t.path), b"state").unwrap();
        fs::write(crate::snapshot::mem_path(&t.path), b"mem").unwrap();
        fs::write(crate::snapshot::meta_path(&t.path), b"not valid json{{{").unwrap();

        assert!(load_one(&t.path, "retired-4").is_none());
    }

    #[test]
    fn load_one_ignores_an_empty_unrelated_directory_without_warning_fields() {
        let t = TempDir::new("empty-dir");
        assert!(load_one(&t.path, "not-a-checkpoint").is_none());
    }

    #[test]
    fn load_one_returns_none_when_metadata_id_does_not_match_directory_name() {
        let t = TempDir::new("id-mismatch");
        fs::write(crate::snapshot::state_path(&t.path), b"state").unwrap();
        fs::write(crate::snapshot::mem_path(&t.path), b"mem").unwrap();
        sample_checkpoint("a-different-id", &t.path).persist(&t.path).unwrap();

        assert!(load_one(&t.path, "dir-name-that-does-not-match").is_none());
    }
}

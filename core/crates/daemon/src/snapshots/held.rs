//! A held snapshot: the `Snapshot` record, its on-disk metadata mirror and the
//! move of its state/memory files into the archive.
//!
//! `persist` writes `meta.json` atomically; `reconcile` (submodule) rebuilds
//! the in-memory set from disk at startup. Path layout and the atomic-write
//! and move primitives live in `snapshots::layout`.

pub mod reconcile;

use crate::snapshots::layout::{mem_path, meta_path, move_file, state_path, write_atomically};
use crate::state::{AttachedDrive, Mount};
use sandkiln_vmm::egress::EgressPolicy;
use sandkiln_vmm::network::Lease;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    /// ends the chain there. See `docs/design-notes/pools-and-lineage.md`
    /// for why a walkable pointer, not a fuller ancestry tree, is the
    /// deliberately narrow first slice.
    pub parent_snapshot_id: Option<String>,
    /// Carried over from `Sandbox::mounts` — see `routes::mounts`. No
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

/// Moves a snapshot's state and memory files from wherever they
/// currently live into `dest_dir` (created if needed), renaming each to
/// this module's own fixed filenames.
///
/// **Deliberately does not touch `snapshot.rootfs_path` at all.**
/// Firecracker bakes the rootfs backing file's absolute host path into
/// `state.snap` itself with no override at resume time, so moving it
/// breaks every future resume/fork — confirmed by a real resume failure
/// when an earlier version of this function moved it too. See the
/// website's Persistence model architecture page (or
/// `docs/design-notes/snapshots.md`, tiered idle lifecycle entry)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshots::test_support::{test_network, TempDir};
    use sandkiln_vmm::vm::NetworkConfig;

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

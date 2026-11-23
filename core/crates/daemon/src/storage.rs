//! Where the daemon keeps its mutable on-disk state.
//!
//! Everything lives under one configurable state directory instead of the
//! OS temp dir: snapshots are documented as durable across restarts, which a
//! tmp cleaner or reboot would silently break. Per-sandbox rootfs files live
//! in their own directory so an operator can put it on the same reflink-capable
//! filesystem (XFS, Btrfs) as the base image and make clones instant.
//!
//! The layout is process-global because snapshot path helpers are called from
//! many places that have no `Config` in hand. It is initialised once in
//! `main` before anything touches disk; tests that never call [`init`] get a
//! temp-dir layout.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct StorageLayout {
    rootfs_dir: PathBuf,
    snapshots_root: PathBuf,
    retired_root: PathBuf,
}

impl StorageLayout {
    /// `state_dir` holds snapshots; `rootfs_dir` holds per-sandbox rootfs files.
    pub fn new(state_dir: &Path, rootfs_dir: &Path) -> Self {
        Self {
            rootfs_dir: rootfs_dir.to_path_buf(),
            snapshots_root: state_dir.join("snapshots"),
            retired_root: state_dir.join("snapshot-history"),
        }
    }

    fn temp_default() -> Self {
        let base = std::env::temp_dir();
        Self::new(&base.join("sandkiln-state"), &base)
    }

    fn create_dirs(&self) -> io::Result<()> {
        for dir in [&self.rootfs_dir, &self.snapshots_root, &self.retired_root] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

static LAYOUT: OnceLock<StorageLayout> = OnceLock::new();

/// Creates the directories and fixes the layout for the process lifetime.
/// Call once, first thing in `main`.
pub fn init(layout: StorageLayout) -> io::Result<()> {
    layout.create_dirs()?;
    LAYOUT.set(layout).map_err(|_| io::Error::other("storage layout already initialised"))
}

fn layout() -> &'static StorageLayout {
    LAYOUT.get_or_init(StorageLayout::temp_default)
}

/// A sandbox's private writable rootfs file.
pub fn rootfs_path(sandbox_id: &str) -> PathBuf {
    layout().rootfs_dir.join(format!("sandkiln-rootfs-{sandbox_id}.ext4"))
}

pub fn snapshots_root() -> PathBuf {
    layout().snapshots_root.clone()
}

pub fn retired_root() -> PathBuf {
    layout().retired_root.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_separates_rootfs_from_snapshot_state() {
        let l = StorageLayout::new(Path::new("/var/sk"), Path::new("/mnt/xfs/rootfs"));
        assert_eq!(l.snapshots_root, Path::new("/var/sk/snapshots"));
        assert_eq!(l.retired_root, Path::new("/var/sk/snapshot-history"));
        assert_eq!(l.rootfs_dir, Path::new("/mnt/xfs/rootfs"));
    }

    #[test]
    fn create_dirs_makes_every_directory() {
        let base = std::env::temp_dir().join(format!("sandkiln-storage-test-{}", std::process::id()));
        let l = StorageLayout::new(&base.join("state"), &base.join("rootfs"));
        l.create_dirs().unwrap();
        assert!(base.join("state/snapshots").is_dir());
        assert!(base.join("state/snapshot-history").is_dir());
        assert!(base.join("rootfs").is_dir());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn rootfs_path_is_keyed_by_sandbox_id() {
        assert!(rootfs_path("abc").to_string_lossy().ends_with("sandkiln-rootfs-abc.ext4"));
    }
}

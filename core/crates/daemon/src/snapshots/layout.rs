//! On-disk layout and durable file primitives shared by held snapshots and
//! retired checkpoints.
//!
//! Owns where a snapshot directory lives (hot root, archive root), the three
//! fixed filenames inside it (`meta.json`, `state.snap`, `mem.bin`),
//! `write_atomically` (never a torn file after a crash) and `move_file`
//! (rename with the cross-filesystem copy fallback).

use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Per-snapshot directories live under the daemon's state directory
/// (`storage::snapshots_root`), never the OS temp dir.
pub fn snapshots_root() -> PathBuf {
    crate::storage::snapshots_root()
}

/// Where one snapshot's state, memory, and metadata files live.
pub fn snapshot_dir(snapshot_id: &str) -> PathBuf {
    snapshots_root().join(snapshot_id)
}

// Shared with `snapshots/retired.rs` (`pub(crate)`) -- a retired checkpoint
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

// `pub(crate)`: `snapshots/retired.rs`'s own retire/restore file movement
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshots::test_support::TempDir;

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
}

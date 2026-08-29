//! Persistent drive management: creating, sizing, and locating standalone
//! ext4 block-device image files that outlive any single sandbox and can
//! be attached to a VM's boot config (see [`crate::vm::DriveConfig`]) —
//! detached from one sandbox and reattached to a later one, unlike the
//! per-sandbox rootfs copy `Vm::boot` writes to in place.
//!
//! Formats new drives as ext4, same as the rootfs images under
//! `images/` — a drive is just an empty filesystem the guest can `mount`,
//! not a differently-shaped artifact.

use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::time::SystemTime;

/// Smallest drive size worth formatting: ext4's own metadata (superblock,
/// inode tables, journal) needs room to exist in, and `mkfs.ext4` fails
/// outright below roughly this size.
pub const MIN_DRIVE_SIZE_MIB: u64 = 16;

/// Manages a directory of persistent drive images, one file per drive,
/// named `<id>.ext4`. Doesn't know anything about which sandbox (if any)
/// has a drive attached — that's tracked by the daemon, which owns the
/// concept of a running sandbox.
pub struct DriveStore {
    dir: PathBuf,
}

pub struct DriveInfo {
    pub id: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub created_at: SystemTime,
}

impl DriveStore {
    /// `dir` should be a location distinct from ephemeral per-sandbox
    /// state (e.g. not `std::env::temp_dir()`) since drives are meant to
    /// persist across sandbox lifetimes. Created if it doesn't exist.
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Creates a new empty drive of `size_mib`, formatted ext4 and ready
    /// to attach. Fails if a drive with this id already exists.
    pub fn create(&self, id: &str, size_mib: u64) -> io::Result<PathBuf> {
        validate_id(id)?;
        if size_mib < MIN_DRIVE_SIZE_MIB {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("drive size must be at least {MIN_DRIVE_SIZE_MIB} MiB, got {size_mib}"),
            ));
        }
        let path = self.path_for(id);
        if path.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("drive already exists: {id}")));
        }

        // A sparse file: mkfs.ext4 only touches the metadata blocks it
        // needs, and actual disk usage grows with what the guest writes —
        // same "let the filesystem do the work" approach as the
        // `cp --reflink=auto` used for rootfs copies.
        let file = fs::File::create(&path)?;
        file.set_len(size_mib * 1024 * 1024)?;
        drop(file);

        match Command::new("mkfs.ext4").arg("-q").arg(&path).status() {
            Ok(status) if status.success() => Ok(path),
            Ok(status) => {
                let _ = fs::remove_file(&path);
                Err(io::Error::other(format!("mkfs.ext4 {path:?} failed: {status}")))
            }
            Err(e) => {
                let _ = fs::remove_file(&path);
                Err(e)
            }
        }
    }

    /// Where a drive with this id lives on disk, whether or not it
    /// actually exists yet.
    pub fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.ext4"))
    }

    pub fn exists(&self, id: &str) -> bool {
        self.path_for(id).is_file()
    }

    /// Lists every drive currently in the store, newest metadata straight
    /// off the filesystem (no separate index to fall out of sync).
    pub fn list(&self) -> io::Result<Vec<DriveInfo>> {
        let mut drives = Vec::new();
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("ext4") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let metadata = entry.metadata()?;
            drives.push(DriveInfo {
                id: id.to_string(),
                path: path.clone(),
                size_bytes: metadata.len(),
                created_at: metadata.created().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
        Ok(drives)
    }

    /// Permanently removes a drive's backing file. Callers are
    /// responsible for making sure it isn't attached to a running
    /// sandbox first — this module has no visibility into that.
    pub fn delete(&self, id: &str) -> io::Result<()> {
        validate_id(id)?;
        fs::remove_file(self.path_for(id))
    }
}

/// Drive ids become both a filename component and a Firecracker
/// `drive_id`, so keep them restricted to characters safe in both — no
/// path separators, no leading dots, nothing that needs escaping.
fn validate_id(id: &str) -> io::Result<()> {
    let valid =
        !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid drive id: {id:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, self-cleaning temp directory per test — these tests do
    /// real file I/O (and `create` shells out to real `mkfs.ext4`, no
    /// KVM/root needed for that), so they need real isolated storage,
    /// not a mock.
    struct TempStore {
        store: DriveStore,
        dir: PathBuf,
    }

    impl TempStore {
        fn new(test_name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sandkiln-drive-test-{test_name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self { store: DriveStore::new(&dir).unwrap(), dir }
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn validate_id_accepts_alphanumeric_hyphen_underscore() {
        assert!(validate_id("abc-123_XYZ").is_ok());
        assert!(validate_id(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn validate_id_rejects_empty_too_long_or_unsafe_characters() {
        assert!(validate_id("").is_err());
        assert!(validate_id(&"a".repeat(65)).is_err());
        assert!(validate_id("../etc/passwd").is_err(), "path traversal must be rejected");
        assert!(validate_id("has space").is_err());
        assert!(validate_id("has.dot").is_err());
    }

    #[test]
    fn path_for_is_deterministic_and_does_not_require_the_drive_to_exist() {
        let t = TempStore::new("path-for");
        assert_eq!(t.store.path_for("abc"), t.dir.join("abc.ext4"));
        assert!(!t.store.exists("abc"));
    }

    #[test]
    fn create_rejects_size_below_minimum() {
        let t = TempStore::new("min-size");
        let err = t.store.create("too-small", MIN_DRIVE_SIZE_MIB - 1).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!t.store.exists("too-small"), "a rejected create must not leave a file behind");
    }

    #[test]
    fn create_rejects_invalid_id_before_touching_the_filesystem() {
        let t = TempStore::new("invalid-id");
        assert!(t.store.create("../escape", MIN_DRIVE_SIZE_MIB).is_err());
        assert!(!t.dir.join("../escape.ext4").exists());
    }

    #[test]
    fn create_then_exists_then_delete_full_cycle() {
        let t = TempStore::new("full-cycle");
        assert!(!t.store.exists("d1"));

        let path = t.store.create("d1", MIN_DRIVE_SIZE_MIB).expect("mkfs.ext4 must be on PATH for this test");
        assert!(t.store.exists("d1"));
        assert_eq!(path, t.store.path_for("d1"));
        assert_eq!(fs::metadata(&path).unwrap().len(), MIN_DRIVE_SIZE_MIB * 1024 * 1024);

        t.store.delete("d1").unwrap();
        assert!(!t.store.exists("d1"));
    }

    #[test]
    fn create_fails_if_id_already_exists() {
        let t = TempStore::new("dup-create");
        t.store.create("d1", MIN_DRIVE_SIZE_MIB).unwrap();
        let err = t.store.create("d1", MIN_DRIVE_SIZE_MIB).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn list_reflects_created_drives_and_ignores_unrelated_files() {
        let t = TempStore::new("list");
        t.store.create("d1", MIN_DRIVE_SIZE_MIB).unwrap();
        t.store.create("d2", MIN_DRIVE_SIZE_MIB).unwrap();
        fs::write(t.dir.join("not-a-drive.txt"), b"ignore me").unwrap();

        let mut ids: Vec<String> = t.store.list().unwrap().into_iter().map(|d| d.id).collect();
        ids.sort();
        assert_eq!(ids, vec!["d1", "d2"]);
    }
}

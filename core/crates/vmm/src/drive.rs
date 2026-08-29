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

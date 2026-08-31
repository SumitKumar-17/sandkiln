//! Managed rootfs images: named, daemon-tracked base filesystems a sandbox
//! can boot from instead of the daemon-wide `SANDKILN_BASE_ROOTFS` default
//! every sandbox used to be stuck with.
//!
//! An image is registered from a path to an already-built ext4 rootfs file
//! already staged on the host filesystem, not accepted as an HTTP upload —
//! accepting an arbitrary multi-gigabyte file over HTTP is a distinct,
//! larger problem (see `images/README.md` and `ROADMAP.md`'s "Base and
//! custom images" section) not attempted here. Registration *copies* that
//! file into a managed directory (mirroring [`crate::drive::DriveStore`]'s
//! "one file per resource, filesystem is the source of truth" pattern)
//! rather than referencing it in place: a later edit, move, or deletion of
//! the original source file can then never corrupt or orphan a sandbox
//! already booted from the registered copy.
//!
//! **This module cannot verify the guest agent is actually baked into a
//! registered image.** That needs loop-mounting the file read-only and
//! inspecting its contents — real root, which the daemon deliberately
//! doesn't have (it runs unprivileged with only ambient `CAP_NET_ADMIN`,
//! see root `AGENTS.md`'s Security section, and there is no way for an
//! already-running unprivileged process to gain root on demand). What
//! [`ImageStore::register`] *can* and does check without any privilege is
//! the ext4 superblock magic — catches "this isn't even an ext4 image"
//! mistakes — but a caller wanting the same guest-agent-baked-in
//! confirmation `scripts/preflight-check.sh --root-checks` already gives
//! for `SANDKILN_BASE_ROOTFS` needs to run that script (now also accepting
//! `--rootfs-image <path>` to check an arbitrary candidate file) out of
//! band, before registering, with real root. See `routes_images::register`
//! on the daemon side for how this is surfaced back to the caller.

use std::fs;
use std::io;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Byte offset of the ext2/3/4 superblock from the start of the
/// filesystem — fixed regardless of block size.
const EXT4_SUPERBLOCK_OFFSET: u64 = 1024;
/// Byte offset of the `s_magic` field within the superblock.
const EXT4_MAGIC_OFFSET_IN_SUPERBLOCK: u64 = 0x38;
/// `s_magic`'s fixed value (`0xEF53`), stored little-endian on disk.
const EXT4_MAGIC_LE: [u8; 2] = [0x53, 0xEF];

/// Manages a directory of registered rootfs images, one file per image,
/// named `<id>.ext4` — structurally identical to
/// [`crate::drive::DriveStore`], just for boot images instead of
/// attachable block devices.
pub struct ImageStore {
    dir: PathBuf,
}

pub struct ImageInfo {
    pub id: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub created_at: SystemTime,
}

impl ImageStore {
    /// `dir` should be a location distinct from ephemeral per-sandbox
    /// state, same reasoning as `DriveStore::new`. Created if it doesn't
    /// exist.
    pub fn new(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Registers `source` (an existing ext4 rootfs file elsewhere on the
    /// host filesystem) under `id`, copying it into this store. Fails if
    /// `id` is invalid or already registered, `source` doesn't exist or
    /// isn't a regular file, `source` doesn't look like an ext4 filesystem
    /// (see the module doc comment for what this check does and doesn't
    /// catch), or the copy itself is incomplete — checked by comparing
    /// source and destination sizes afterward rather than trusting `cp`'s
    /// exit status alone, since a near-full disk can make a copy come up
    /// short of its source size while `cp` still exits `0` (a real failure
    /// mode this project has hit before with a truncated native binary
    /// during a disk-full `npm install`, see root `AGENTS.md`'s Development
    /// Environment gotchas).
    pub fn register(&self, id: &str, source: &Path) -> io::Result<PathBuf> {
        validate_id(id)?;

        let source_metadata = fs::metadata(source).map_err(|e| {
            io::Error::new(e.kind(), format!("image source path does not exist or is not accessible: {source:?}: {e}"))
        })?;
        if !source_metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("image source path is not a regular file: {source:?}"),
            ));
        }

        if !looks_like_ext4(source)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{source:?} does not look like an ext4 filesystem (no ext4 superblock magic found at byte {}) \
                     — sandkiln images must be pre-built ext4 rootfs files, see images/build-universal-image.sh",
                    EXT4_SUPERBLOCK_OFFSET + EXT4_MAGIC_OFFSET_IN_SUPERBLOCK
                ),
            ));
        }

        let dest = self.path_for(id);
        if dest.exists() {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, format!("image already exists: {id}")));
        }

        // Same `cp --reflink=auto` approach as `routes_sandbox::clone_rootfs`
        // — an instant copy-on-write clone on a filesystem that supports it
        // (XFS, Btrfs), an ordinary copy elsewhere (ext4).
        let status = Command::new("cp").arg("--reflink=auto").arg(source).arg(&dest).status();
        let status = match status {
            Ok(status) => status,
            Err(e) => {
                let _ = fs::remove_file(&dest);
                return Err(e);
            }
        };
        if !status.success() {
            let _ = fs::remove_file(&dest);
            return Err(io::Error::other(format!("cp --reflink=auto {source:?} {dest:?} failed: {status}")));
        }

        let dest_size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        if dest_size != source_metadata.len() {
            let _ = fs::remove_file(&dest);
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!(
                    "copy of {source:?} into the image store is incomplete ({dest_size} of {} bytes) — \
                     check available disk space under {:?}",
                    source_metadata.len(),
                    self.dir
                ),
            ));
        }

        Ok(dest)
    }

    /// Where an image with this id lives on disk, whether or not it
    /// actually exists yet.
    pub fn path_for(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.ext4"))
    }

    pub fn exists(&self, id: &str) -> bool {
        self.path_for(id).is_file()
    }

    /// Lists every image currently in the store, straight off the
    /// filesystem — no separate index to fall out of sync, same as
    /// `DriveStore::list`.
    pub fn list(&self) -> io::Result<Vec<ImageInfo>> {
        let mut images = Vec::new();
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
            images.push(ImageInfo {
                id: id.to_string(),
                path: path.clone(),
                size_bytes: metadata.len(),
                created_at: metadata.created().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
        Ok(images)
    }

    /// Permanently removes an image's backing file. Callers are
    /// responsible for making sure no sandbox or snapshot still references
    /// it first — this module has no visibility into that (see
    /// `AppState::image_holder` on the daemon side).
    pub fn delete(&self, id: &str) -> io::Result<()> {
        validate_id(id)?;
        fs::remove_file(self.path_for(id))
    }
}

/// Image ids become a filename component, same restriction as
/// `drive::validate_id` for the same reason — no path separators, no
/// leading dots, nothing that needs escaping.
fn validate_id(id: &str) -> io::Result<()> {
    let valid =
        !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidInput, format!("invalid image id: {id:?}")))
    }
}

/// Best-effort, unprivileged check that `path` at least looks like an
/// ext2/3/4 filesystem (they share the same superblock layout and magic):
/// reads the two magic bytes fixed at byte offset 1080 from the start of
/// the file, present regardless of block size. Doesn't and can't confirm
/// anything about the filesystem's *contents* — see the module doc
/// comment for why that needs root this daemon doesn't have.
fn looks_like_ext4(path: &Path) -> io::Result<bool> {
    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(EXT4_SUPERBLOCK_OFFSET + EXT4_MAGIC_OFFSET_IN_SUPERBLOCK))?;
    let mut magic = [0u8; 2];
    match file.read_exact(&mut magic) {
        Ok(()) => Ok(magic == EXT4_MAGIC_LE),
        // Too small to even contain a superblock — definitely not ext4,
        // not an error worth surfacing differently than "no match".
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, self-cleaning temp directory per test — real file I/O (and
    /// `register` shells out to real `cp`, no KVM/root needed for that),
    /// matching `drive::tests::TempStore`'s convention.
    struct TempStore {
        store: ImageStore,
        dir: PathBuf,
    }

    impl TempStore {
        fn new(test_name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sandkiln-image-test-{test_name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self { store: ImageStore::new(&dir).unwrap(), dir }
        }

        fn scratch_dir(&self) -> PathBuf {
            let dir = self.dir.join("scratch");
            fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// Builds a minimal file with a correct-looking ext4 superblock magic
    /// at the right offset, without needing a real `mkfs.ext4` (that needs
    /// a real block device-sized file and is exercised for real in
    /// `drive::tests`) — just enough bytes for `looks_like_ext4` to find
    /// what it's looking for.
    fn write_fake_ext4(path: &Path) {
        let mut bytes = vec![0u8; 2048];
        let magic_offset = (EXT4_SUPERBLOCK_OFFSET + EXT4_MAGIC_OFFSET_IN_SUPERBLOCK) as usize;
        bytes[magic_offset..magic_offset + 2].copy_from_slice(&EXT4_MAGIC_LE);
        fs::write(path, bytes).unwrap();
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
    fn looks_like_ext4_true_for_correct_magic_at_correct_offset() {
        let dir = std::env::temp_dir().join(format!("sandkiln-image-magic-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake.ext4");
        write_fake_ext4(&path);
        assert!(looks_like_ext4(&path).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn looks_like_ext4_false_for_wrong_magic() {
        let dir = std::env::temp_dir().join(format!("sandkiln-image-magic-wrong-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("not-ext4.bin");
        fs::write(&path, vec![0u8; 2048]).unwrap();
        assert!(!looks_like_ext4(&path).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn looks_like_ext4_false_for_a_file_too_small_to_hold_a_superblock() {
        let dir = std::env::temp_dir().join(format!("sandkiln-image-magic-small-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.bin");
        fs::write(&path, b"way too small").unwrap();
        assert!(!looks_like_ext4(&path).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn register_rejects_invalid_id_before_touching_the_filesystem() {
        let t = TempStore::new("invalid-id");
        let source = t.scratch_dir().join("src.ext4");
        write_fake_ext4(&source);
        assert!(t.store.register("../escape", &source).is_err());
        assert!(!t.dir.join("../escape.ext4").exists());
    }

    #[test]
    fn register_rejects_a_missing_source() {
        let t = TempStore::new("missing-source");
        let err = t.store.register("img1", &t.scratch_dir().join("does-not-exist.ext4")).unwrap_err();
        assert!(!t.store.exists("img1"));
        assert_ne!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn register_rejects_a_directory_as_source() {
        let t = TempStore::new("dir-source");
        let dir_as_source = t.scratch_dir();
        let err = t.store.register("img1", &dir_as_source).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!t.store.exists("img1"));
    }

    #[test]
    fn register_rejects_a_file_that_does_not_look_like_ext4() {
        let t = TempStore::new("not-ext4");
        let source = t.scratch_dir().join("plain.txt");
        fs::write(&source, b"just some text, not a filesystem").unwrap();
        let err = t.store.register("img1", &source).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(!t.store.exists("img1"));
    }

    #[test]
    fn register_then_exists_then_delete_full_cycle() {
        let t = TempStore::new("full-cycle");
        let source = t.scratch_dir().join("src.ext4");
        write_fake_ext4(&source);

        assert!(!t.store.exists("img1"));
        let path = t.store.register("img1", &source).unwrap();
        assert!(t.store.exists("img1"));
        assert_eq!(path, t.store.path_for("img1"));
        assert_eq!(fs::read(&path).unwrap(), fs::read(&source).unwrap());

        t.store.delete("img1").unwrap();
        assert!(!t.store.exists("img1"));
    }

    #[test]
    fn register_fails_if_id_already_exists() {
        let t = TempStore::new("dup-register");
        let source = t.scratch_dir().join("src.ext4");
        write_fake_ext4(&source);
        t.store.register("img1", &source).unwrap();
        let err = t.store.register("img1", &source).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn register_does_not_consume_or_move_the_source_file() {
        let t = TempStore::new("source-untouched");
        let source = t.scratch_dir().join("src.ext4");
        write_fake_ext4(&source);
        t.store.register("img1", &source).unwrap();
        assert!(source.exists(), "the original source file must be left in place — register copies, never moves");
    }

    #[test]
    fn list_reflects_registered_images_and_ignores_unrelated_files() {
        let t = TempStore::new("list");
        let source = t.scratch_dir().join("src.ext4");
        write_fake_ext4(&source);
        t.store.register("img1", &source).unwrap();
        t.store.register("img2", &source).unwrap();
        fs::write(t.dir.join("not-an-image.txt"), b"ignore me").unwrap();

        let mut ids: Vec<String> = t.store.list().unwrap().into_iter().map(|i| i.id).collect();
        ids.sort();
        assert_eq!(ids, vec!["img1", "img2"]);
    }

    #[test]
    fn delete_rejects_invalid_id_before_touching_the_filesystem() {
        let t = TempStore::new("delete-invalid-id");
        assert!(t.store.delete("../escape").is_err());
    }

    #[test]
    fn delete_nonexistent_image_is_not_found() {
        let t = TempStore::new("delete-missing");
        let err = t.store.delete("does-not-exist").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}

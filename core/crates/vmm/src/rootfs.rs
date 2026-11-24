//! Per-sandbox rootfs provisioning: the one place that clones a base image
//! (or snapshot disk) into a fresh writable file.
//!
//! A clone is a CoW reflink (`FICLONE`) when source and destination share a
//! reflink-capable filesystem (XFS, Btrfs), and an in-kernel byte copy
//! otherwise. The method is *returned*, never hidden: a silent fallback to a
//! full copy is indistinguishable from "slow disk" in latency numbers, which
//! is exactly how an earlier benchmark was misread (see docs/benchmarks.md).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

/// `_IOW(0x94, 9, int)` from `linux/fs.h`.
const FICLONE: libc::c_ulong = 0x4004_9409;

/// How a clone was actually performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloneMethod {
    /// Instant copy-on-write clone; no data was copied.
    Reflink,
    /// Full byte copy (reflink unsupported across these paths/filesystem).
    Copy,
}

impl CloneMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            CloneMethod::Reflink => "reflink",
            CloneMethod::Copy => "copy",
        }
    }
}

/// Clones `src` to a new file `dst`. Fails if `dst` already exists; on any
/// error no partial `dst` is left behind.
pub fn clone_file(src: &Path, dst: &Path) -> io::Result<CloneMethod> {
    let mut src_file = File::open(src)?;
    let mode = src_file.metadata()?.permissions();
    let mut dst_file = OpenOptions::new().write(true).create_new(true).open(dst)?;

    let result = clone_into(&mut src_file, &mut dst_file).and_then(|method| {
        fs::set_permissions(dst, mode)?;
        Ok(method)
    });
    if result.is_err() {
        let _ = fs::remove_file(dst);
    }
    result
}

fn clone_into(src: &mut File, dst: &mut File) -> io::Result<CloneMethod> {
    // SAFETY: both fds are valid for the duration of the call; FICLONE takes
    // the source fd as its argument and does not retain either.
    let rc = unsafe { libc::ioctl(dst.as_raw_fd(), FICLONE, src.as_raw_fd()) };
    if rc == 0 {
        return Ok(CloneMethod::Reflink);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        // Unsupported filesystem, cross-device, or not a reflink-able file:
        // all mean "fall back to a copy", anything else is a real failure.
        Some(libc::EOPNOTSUPP) | Some(libc::EXDEV) | Some(libc::EINVAL) | Some(libc::ENOTTY) | Some(libc::ENOSYS) => {
            io::copy(src, dst)?;
            Ok(CloneMethod::Copy)
        }
        _ => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sandkiln-rootfs-test-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn clone_preserves_contents_and_reports_a_method() {
        let dir = scratch("contents");
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        fs::write(&src, b"base image bytes").unwrap();
        let method = clone_file(&src, &dst).unwrap();
        assert!(matches!(method, CloneMethod::Reflink | CloneMethod::Copy));
        assert_eq!(fs::read(&dst).unwrap(), b"base image bytes");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clone_is_independent_of_the_source_afterwards() {
        let dir = scratch("independent");
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        fs::write(&src, b"original").unwrap();
        clone_file(&src, &dst).unwrap();
        fs::write(&dst, b"mutated!").unwrap();
        assert_eq!(fs::read(&src).unwrap(), b"original");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clone_refuses_to_overwrite_an_existing_destination() {
        let dir = scratch("exists");
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        fs::write(&src, b"a").unwrap();
        fs::write(&dst, b"keep me").unwrap();
        assert_eq!(clone_file(&src, &dst).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&dst).unwrap(), b"keep me");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_source_fails_and_leaves_no_destination() {
        let dir = scratch("missing");
        let dst = dir.join("dst");
        assert!(clone_file(&dir.join("nope"), &dst).is_err());
        assert!(!dst.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clone_preserves_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("mode");
        let (src, dst) = (dir.join("src"), dir.join("dst"));
        fs::write(&src, b"a").unwrap();
        fs::set_permissions(&src, fs::Permissions::from_mode(0o640)).unwrap();
        clone_file(&src, &dst).unwrap();
        assert_eq!(fs::metadata(&dst).unwrap().permissions().mode() & 0o777, 0o640);
        fs::remove_dir_all(dir).unwrap();
    }
}

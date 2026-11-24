//! The jail's directory tree: where it lives, and how host files (kernel,
//! rootfs, drives, binaries) are placed inside it before jailer starts.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Linux's fixed `EXDEV` errno (cross-device link) — checked by raw code
/// rather than pulling in `libc` for one constant this project has no
/// other use for.
const EXDEV: i32 = 18;

/// Firecracker's own directory convention: `<chroot_base>/<exec file's
/// basename>/<id>/root`. Jailer creates and chowns this tree for the exec
/// file it hard-links in, but any other resource (kernel, rootfs, drives,
/// a resumed snapshot's memory/state files) must exist inside it *before*
/// jailer runs.
pub fn chroot_root(chroot_base_dir: &Path, firecracker_bin: &Path, jail_instance_id: &str) -> PathBuf {
    let exec_name = firecracker_bin.file_name().expect("firecracker_bin must have a file name");
    chroot_base_dir.join(exec_name).join(jail_instance_id).join("root")
}

/// The instance directory jailer owns for one VM — `chroot_root`'s
/// parent. Removing it on VM stop tears down everything jailer created
/// for that VM, not just the chroot itself.
pub fn instance_dir(chroot_root: &Path) -> PathBuf {
    chroot_root.parent().expect("chroot_root is always <base>/<exec>/<id>/root, so it always has a parent").to_path_buf()
}

/// Creates `chroot_root` if missing and makes it traversable by any uid
/// (`o+x` — lookup by exact filename, all firecracker needs). Jailer
/// applies its own ownership/permissions later, but resources link in
/// *before* jailer runs, so this must be usable pre-emptively.
pub fn prepare_chroot_dir(chroot_root: &Path) -> io::Result<()> {
    fs::create_dir_all(chroot_root)?;
    let mut perms = fs::metadata(chroot_root)?.permissions();
    perms.set_mode(perms.mode() | 0o711);
    fs::set_permissions(chroot_root, perms)
}

/// One resource (kernel image, rootfs, a drive, or a snapshot's
/// memory/state file) placed inside a VM's chroot.
#[derive(Debug)]
pub struct JailedPath {
    /// Where the linked/copied file lives on the host, inside the
    /// chroot — removed automatically when the instance directory is
    /// torn down.
    pub host_path: PathBuf,
    /// The path firecracker (running chrooted) must use — always rooted
    /// at `/`, the chroot's own root from firecracker's point of view.
    pub in_jail_path: PathBuf,
}

/// Makes `host_source` reachable inside `chroot_root` at
/// `chroot_root/<jail_relative_name>`, returning both the host path and
/// the path firecracker must use to open it.
///
/// Hard-links when possible (instant, no extra disk — matters for the
/// rootfs specifically), falls back to a copy across filesystem
/// boundaries (`EXDEV`), same as jailer's own exec-file handling.
///
/// Across filesystems a hard link is impossible. `allow_copy` says whether a
/// copy is an acceptable substitute: it is for a disposable or read-only
/// resource, but never for a persistent drive Firecracker writes to, where
/// the writes would land in a copy that is deleted with the jail and the
/// drive's data would be silently lost; then this fails instead.
///
/// A read-only resource is made world-readable. A `writable` one (the
/// rootfs, a drive) gets the jail group (`jail_gid`) and group read-write,
/// never world-writable and never `chown`ed to the per-VM uid: a hard link
/// shares one inode with the source, so changing its owner would change the
/// source's too. The group change needs the daemon's user to be a member of
/// the jail group, which it must be anyway to reach the jail.
pub fn link_resource_into_jail(
    host_source: &Path,
    chroot_root: &Path,
    jail_relative_name: &str,
    writable: bool,
    jail_gid: u32,
    allow_copy: bool,
) -> io::Result<JailedPath> {
    prepare_chroot_dir(chroot_root)?;
    let host_dest = chroot_root.join(jail_relative_name);
    let _ = fs::remove_file(&host_dest);

    match fs::hard_link(host_source, &host_dest) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(EXDEV) && allow_copy => {
            fs::copy(host_source, &host_dest)?;
        }
        Err(e) if e.raw_os_error() == Some(EXDEV) => {
            return Err(io::Error::other(format!(
                "{} is on a different filesystem than the jail directory, and a copy would lose everything written to it; \
                 put the jail base directory (jailer_chroot_base_dir) and the drives directory on the same filesystem",
                host_source.display()
            )));
        }
        Err(e) => return Err(e),
    }

    let mut perms = fs::metadata(&host_dest)?.permissions();
    if writable {
        std::os::unix::fs::chown(&host_dest, None, Some(jail_gid))?;
        perms.set_mode(perms.mode() | 0o060);
    } else {
        perms.set_mode(perms.mode() | 0o004);
    }
    fs::set_permissions(&host_dest, perms)?;

    Ok(JailedPath { host_path: host_dest, in_jail_path: PathBuf::from("/").join(jail_relative_name) })
}

/// Puts a private copy of an executable in the jail, readable and
/// executable by every uid. A copy, not a hard link: two jails sharing one
/// inode would share the binary's page cache, which is a channel between
/// sandboxes that the per-VM uid and chroot are there to remove (jailer
/// copies its own exec file for the same reason).
pub fn copy_binary_into_jail(host_source: &Path, chroot_root: &Path, jail_relative_name: &str) -> io::Result<JailedPath> {
    prepare_chroot_dir(chroot_root)?;
    let host_dest = chroot_root.join(jail_relative_name);
    let _ = fs::remove_file(&host_dest);
    fs::copy(host_source, &host_dest)?;
    fs::set_permissions(&host_dest, fs::Permissions::from_mode(0o755))?;
    Ok(JailedPath { host_path: host_dest, in_jail_path: PathBuf::from("/").join(jail_relative_name) })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TempDir;
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn chroot_root_follows_the_exec_file_name_then_id_then_root() {
        let root = chroot_root(Path::new("/srv/jailer"), Path::new("/opt/sandkiln-firecracker-supervisor"), "sandkiln-7");
        assert_eq!(root, PathBuf::from("/srv/jailer/sandkiln-firecracker-supervisor/sandkiln-7/root"));
    }

    #[test]
    fn instance_dir_is_chroot_roots_parent() {
        let root = PathBuf::from("/srv/jailer/firecracker/sandkiln-7/root");
        assert_eq!(instance_dir(&root), PathBuf::from("/srv/jailer/firecracker/sandkiln-7"));
    }


    #[test]
    fn prepare_chroot_dir_creates_missing_parents_and_sets_other_execute() {
        let tmp = TempDir::new("prepare-chroot");
        let root = tmp.path.join("firecracker").join("sandkiln-1").join("root");
        prepare_chroot_dir(&root).unwrap();
        assert!(root.is_dir());
        let mode = fs::metadata(&root).unwrap().permissions().mode();
        assert_eq!(mode & 0o711, 0o711);
    }

    #[test]
    fn link_resource_into_jail_hard_links_and_reports_the_in_jail_path() {
        let tmp = TempDir::new("link-resource");
        let source = tmp.path.join("kernel-image");
        fs::write(&source, b"pretend kernel bytes").unwrap();
        let chroot_root = tmp.path.join("firecracker").join("sandkiln-2").join("root");

        let linked = link_resource_into_jail(&source, &chroot_root, "kernel", false, own_gid(&source), true).unwrap();

        assert_eq!(linked.in_jail_path, PathBuf::from("/kernel"));
        assert_eq!(linked.host_path, chroot_root.join("kernel"));
        assert_eq!(fs::read(&linked.host_path).unwrap(), b"pretend kernel bytes");

        let source_meta = fs::metadata(&source).unwrap();
        let dest_meta = fs::metadata(&linked.host_path).unwrap();
        assert_eq!(source_meta.ino(), dest_meta.ino(), "same filesystem must hard-link, not copy");
    }

    #[test]
    fn link_resource_into_jail_makes_read_only_resources_world_readable_not_writable() {
        let tmp = TempDir::new("link-readonly");
        let source = tmp.path.join("rootfs-ro.ext4");
        fs::write(&source, b"ro").unwrap();
        let chroot_root = tmp.path.join("firecracker").join("sandkiln-3").join("root");

        let linked = link_resource_into_jail(&source, &chroot_root, "rootfs.ext4", false, own_gid(&source), true).unwrap();
        let mode = fs::metadata(&linked.host_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o007, 0o004, "read-only resource must be world-readable, not world-writable");
    }

    /// A group the test user may chgrp to: the group of a file it just created.
    fn own_gid(file: &Path) -> u32 {
        fs::metadata(file).unwrap().gid()
    }

    #[test]
    fn link_resource_into_jail_makes_writable_resources_group_read_write_never_world_writable() {
        let tmp = TempDir::new("link-writable");
        let source = tmp.path.join("rootfs-rw.ext4");
        fs::write(&source, b"rw").unwrap();
        let chroot_root = tmp.path.join("firecracker").join("sandkiln-4").join("root");

        let linked = link_resource_into_jail(&source, &chroot_root, "rootfs.ext4", true, own_gid(&source), true).unwrap();
        let mode = fs::metadata(&linked.host_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o060, 0o060, "writable resource (the rootfs) must be group read+write");
        assert_eq!(mode & 0o002, 0, "and never world-writable");
    }

    /// A temp directory on a different filesystem than `std::env::temp_dir()`,
    /// or `None` when the host has only one (then the cross-device case
    /// cannot be built).
    fn other_filesystem_dir() -> Option<PathBuf> {
        let shm = PathBuf::from("/dev/shm");
        let here = fs::metadata(std::env::temp_dir()).ok()?.dev();
        (shm.is_dir() && fs::metadata(&shm).ok()?.dev() != here).then_some(shm)
    }

    #[test]
    fn across_filesystems_a_copy_is_made_when_allowed_and_refused_when_not() {
        let Some(other) = other_filesystem_dir() else { return };
        let source = other.join(format!("sandkiln-exdev-test-{}", std::process::id()));
        fs::write(&source, b"drive bytes").unwrap();
        let tmp = TempDir::new("exdev");
        let root = tmp.path.join("firecracker").join("sandkiln-8").join("root");

        let copied = link_resource_into_jail(&source, &root, "d.ext4", false, own_gid(&source), true).unwrap();
        assert_ne!(fs::metadata(&source).unwrap().ino(), fs::metadata(&copied.host_path).unwrap().ino());

        let err = link_resource_into_jail(&source, &root, "d2.ext4", true, own_gid(&source), false).unwrap_err();
        let _ = fs::remove_file(&source);
        assert!(err.to_string().contains("different filesystem") && err.to_string().contains("jailer_chroot_base_dir"), "{err}");
        assert!(!root.join("d2.ext4").exists(), "no partial copy may be left behind");
    }

    #[test]
    fn link_resource_into_jail_does_not_change_the_sources_own_permissions_destructively() {
        // Regression guard: chowning a hard link (instead of chmod-adding
        // a bit) would silently mutate the *source* file's ownership too,
        // since they share one inode.
        let tmp = TempDir::new("link-source-untouched");
        let source = tmp.path.join("shared-kernel");
        fs::write(&source, b"shared").unwrap();
        let original_mode = fs::metadata(&source).unwrap().permissions().mode();

        let chroot_root = tmp.path.join("firecracker").join("sandkiln-5").join("root");
        link_resource_into_jail(&source, &chroot_root, "kernel", false, own_gid(&source), true).unwrap();

        // Mode bits are only ever widened (adding other-read), never replaced.
        let after_mode = fs::metadata(&source).unwrap().permissions().mode();
        assert_eq!(after_mode & 0o700, original_mode & 0o700, "owner permission bits must be unchanged");
    }


    #[test]
    fn copy_binary_into_jail_makes_a_private_executable_copy() {
        let tmp = TempDir::new("copy-binary");
        let source = tmp.path.join("firecracker");
        fs::write(&source, b"binary").unwrap();
        let root = tmp.path.join("jail").join("root");
        let copied = copy_binary_into_jail(&source, &root, "firecracker").unwrap();
        assert_eq!(copied.in_jail_path, PathBuf::from("/firecracker"));
        assert_ne!(fs::metadata(&source).unwrap().ino(), fs::metadata(&copied.host_path).unwrap().ino(), "must not share an inode");
        assert_eq!(fs::metadata(&copied.host_path).unwrap().permissions().mode() & 0o777, 0o755);
    }
}

//! Firecracker's jailer: re-execs the `firecracker` binary inside a
//! chroot'd, cgroup-limited environment running as a dedicated
//! unprivileged uid/gid, instead of the direct `Command::new(firecracker_bin)`
//! spawn `vm::boot` otherwise uses. See `ROADMAP.md`'s "Security
//! hardening" section and `SELF_HOSTING.md`'s jailer setup steps.
//!
//! ## Why the daemon can't just do this itself
//!
//! Everything jailer does — chroot(2), setuid/setgid, creating device
//! nodes for `/dev/kvm`/`/dev/net/tun` inside the jail, cgroup
//! management — needs privileges the daemon deliberately doesn't have
//! (it runs unprivileged with only ambient `CAP_NET_ADMIN`, see root
//! `AGENTS.md` and `SELF_HOSTING.md`'s "Why not just run as root"). Jailer
//! itself has to run with those privileges for the brief setup window
//! before it drops all of them and execs `firecracker` as the target
//! uid/gid — the standard way to give an unprivileged daemon access to
//! that is to make the `jailer` binary itself setuid-root (a small,
//! purpose-built binary, not the whole daemon). See `SELF_HOSTING.md`.
//!
//! ## What crosses the chroot boundary
//!
//! Once jailer calls `chroot()`, the firecracker process it execs can no
//! longer see any host path outside its jail root — every file the VM
//! config references (kernel image, rootfs, extra drives) has to already
//! exist inside the jail *before* jailer starts, and every path handed to
//! Firecracker's own API afterward (`/boot-source`'s `kernel_image_path`,
//! `/drives/*`'s `path_on_host`, `/vsock`'s `uds_path`) has to be the
//! in-jail path, not the real host path. `link_resource_into_jail` is
//! what makes a host file appear inside the jail; `vm::boot_inner` is what
//! rewrites the API bodies to use the resulting in-jail paths.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::ops::RangeInclusive;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Linux's fixed `EXDEV` errno (cross-device link) — checked by raw code
/// rather than pulling in `libc` for one constant this project has no
/// other use for.
const EXDEV: i32 = 18;

/// One VM's fully-resolved jail launch parameters — everything
/// `vm::boot_inner`/`vm::resume` need to invoke jailer for one microVM.
/// `uid`/`gid` must come from a [`JailerIdPool`] (or an equivalent
/// allocation the caller guarantees is unique among concurrently running
/// jailed VMs); nothing here re-validates that.
#[derive(Clone)]
pub struct JailLaunch {
    pub jailer_bin: PathBuf,
    pub chroot_base_dir: PathBuf,
    pub uid: u32,
    pub gid: u32,
}

/// Hands out distinct uid/gid pairs to concurrent jailed VMs from a fixed
/// range, and takes them back on release. Mirrors
/// `crate::network::NetworkManager`'s tap/IP pool: a bounded, pre-declared
/// id space rather than computing one on the fly (e.g. hashing a sandbox
/// id into a uid), because a collision here isn't just a logic bug — two
/// jailed VMs sharing a uid means one guest's escaped process can
/// `kill`/`ptrace`/read files left world-readable-to-owner by the other,
/// defeating the entire point of per-VM uid separation.
///
/// The configured range should sit outside normal system/user uids —
/// recommend 600000 and above, mirroring the subordinate-uid ranges
/// `/etc/subuid` conventionally uses — so a jailed VM's uid can never
/// collide with a real host account. The same numeric id is used for both
/// uid and gid; jailer accepts them independently, but a shared pool
/// keyed by one number is simpler to reason about and there's no reason
/// here for a VM's group to be shared with anything else.
pub struct JailerIdPool {
    free: Mutex<VecDeque<u32>>,
}

impl JailerIdPool {
    pub fn new(range: RangeInclusive<u32>) -> Self {
        Self { free: Mutex::new(range.collect()) }
    }

    /// Number of ids currently available to lease — the daemon's max
    /// concurrent-jailed-sandbox ceiling at this instant.
    pub fn available(&self) -> usize {
        self.free.lock().unwrap().len()
    }

    pub fn lease(&self) -> io::Result<u32> {
        self.free.lock().unwrap().pop_front().ok_or_else(|| io::Error::other("no free jailer uid/gid left in the pool"))
    }

    pub fn release(&self, id: u32) {
        self.free.lock().unwrap().push_back(id);
    }
}

/// A stable, jailer-`--id`-safe identifier for one VM, derived from the
/// same monotonic counter `vm::Vm` already uses for its socket paths —
/// keeps every id jailer/chroot/socket path this module touches traceable
/// back to one number in the logs.
pub fn jail_instance_id(vm_id: u64) -> String {
    format!("sandkiln-{vm_id}")
}

/// Firecracker's own directory-naming convention: `<chroot_base>/<exec
/// file's basename>/<id>/root`. Jailer creates and chowns this tree
/// itself for the exec file it hard-links in, but any additional
/// resource (kernel image, rootfs, extra drives, or — for a resumed
/// snapshot — the memory/state files) must already exist inside it before
/// jailer is invoked, since firecracker cannot see anything outside its
/// new root after jailer calls `chroot()`.
pub fn chroot_root(chroot_base_dir: &Path, firecracker_bin: &Path, jail_instance_id: &str) -> PathBuf {
    let exec_name = firecracker_bin.file_name().expect("firecracker_bin must have a file name");
    chroot_base_dir.join(exec_name).join(jail_instance_id).join("root")
}

/// The instance directory jailer owns for one VM — `chroot_root`'s
/// parent. Removing this on VM stop tears down everything jailer created
/// for that VM (the `root/` chroot and anything else jailer keeps
/// alongside it), not just the chroot itself.
pub fn instance_dir(chroot_root: &Path) -> PathBuf {
    chroot_root.parent().expect("chroot_root is always <base>/<exec>/<id>/root, so it always has a parent").to_path_buf()
}

/// Creates `chroot_root` if it doesn't exist and makes it traversable by
/// any uid (`o+x` — lookup by exact filename, not directory listing,
/// which is all firecracker itself ever needs). Jailer applies its own
/// ownership/permissions to this tree when it starts, but resources are
/// linked in *before* jailer runs, so this has to be usable pre-emptively
/// regardless of exactly what jailer does to it afterward.
pub fn prepare_chroot_dir(chroot_root: &Path) -> io::Result<()> {
    fs::create_dir_all(chroot_root)?;
    let mut perms = fs::metadata(chroot_root)?.permissions();
    perms.set_mode(perms.mode() | 0o711);
    fs::set_permissions(chroot_root, perms)
}

/// One resource (kernel image, rootfs, a drive, or a snapshot's
/// memory/state file) placed inside a VM's chroot.
pub struct JailedPath {
    /// Where the linked/copied file actually lives on the host — inside
    /// the chroot, so also removed automatically when the instance
    /// directory is torn down on VM stop.
    pub host_path: PathBuf,
    /// The path firecracker itself (running chrooted) must use to reach
    /// the same file — always rooted at `/`, since that's the chroot's
    /// own root from firecracker's point of view.
    pub in_jail_path: PathBuf,
}

/// Makes `host_source` reachable inside `chroot_root` at
/// `chroot_root/<jail_relative_name>`, and returns both the resulting
/// host path and the path firecracker itself must use to open it.
///
/// Hard-links when possible (instant, no extra disk space — matters for
/// the rootfs specifically, see root `AGENTS.md`'s rootfs-copy-latency
/// history) and falls back to a real copy across filesystem boundaries
/// (`EXDEV`), the same way jailer's own handling of the exec file does.
///
/// The link/copy is made world-readable (and world-writable for
/// `writable` resources, i.e. the rootfs) rather than `chown`ed to the
/// jail's uid/gid: a hard link shares one inode with the source file, so
/// `chown`ing it would silently change the *source* file's ownership too
/// (a shared kernel image, or another sandbox's still-held drive) —
/// permission bits scoped to "other" give the jailed uid (whatever it
/// turns out to be) access without touching ownership of anything shared.
pub fn link_resource_into_jail(
    host_source: &Path,
    chroot_root: &Path,
    jail_relative_name: &str,
    writable: bool,
) -> io::Result<JailedPath> {
    prepare_chroot_dir(chroot_root)?;
    let host_dest = chroot_root.join(jail_relative_name);
    let _ = fs::remove_file(&host_dest);

    match fs::hard_link(host_source, &host_dest) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(EXDEV) => {
            fs::copy(host_source, &host_dest)?;
        }
        Err(e) => return Err(e),
    }

    let other_bits = if writable { 0o006 } else { 0o004 };
    let mut perms = fs::metadata(&host_dest)?.permissions();
    perms.set_mode(perms.mode() | other_bits);
    fs::set_permissions(&host_dest, perms)?;

    Ok(JailedPath { host_path: host_dest, in_jail_path: PathBuf::from("/").join(jail_relative_name) })
}

/// A conservative cgroup v2 memory ceiling for a VM configured with
/// `mem_size_mib` of guest RAM: the guest's own configured memory plus
/// headroom for Firecracker's own VMM process overhead (page tables,
/// virtio queue buffers, the vsock/balloon backends). Without this
/// margin, a VM sized right at its cgroup ceiling gets OOM-killed by the
/// kernel before the guest even finishes booting — the ceiling has to
/// bound the whole jailed process, not just what the guest thinks its RAM
/// is.
pub fn cgroup_memory_max_bytes(mem_size_mib: u32) -> u64 {
    const VMM_OVERHEAD_MIB: u64 = 128;
    (mem_size_mib as u64 + VMM_OVERHEAD_MIB) * 1024 * 1024
}

/// cgroup v2's `cpu.max` value is `"<quota> <period>"` in microseconds —
/// this pins one jailed VM to no more than `vcpu_count` fully-utilized
/// host cores' worth of CPU time. Without it a runaway guest workload in
/// one sandbox can starve every other sandbox's vCPU threads on a shared
/// host, since vCPU threads are ordinary host threads with no scheduling
/// isolation of their own beyond this.
pub fn cgroup_cpu_max(vcpu_count: u8) -> String {
    const PERIOD_US: u64 = 100_000;
    format!("{} {PERIOD_US}", vcpu_count as u64 * PERIOD_US)
}

/// The `--cgroup <controller>.<key>=<value>` values to pass to jailer for
/// one VM's resource ceiling, derived from the same `vcpu_count`/
/// `mem_size_mib` already used for Firecracker's own `/machine-config`.
pub fn cgroup_limits(mem_size_mib: u32, vcpu_count: u8) -> Vec<String> {
    vec![format!("memory.max={}", cgroup_memory_max_bytes(mem_size_mib)), format!("cpu.max={}", cgroup_cpu_max(vcpu_count))]
}

/// Builds the full jailer argv (excluding argv[0], which is
/// `launch.jailer_bin` itself) for launching one VM. Pure and
/// independently testable — `vm::boot_inner`/`vm::resume` just pass this
/// straight to `Command::args`.
///
/// Deliberately omits `--daemonize`: without it, jailer stays attached to
/// its parent and directly `exec`s firecracker in place once it's done
/// setting up (rather than forking, detaching, and exiting) — the `Child`
/// handle `Command::spawn` returns for the jailer invocation therefore
/// *becomes* the firecracker process after exec, under the same pid, so
/// the existing `child.kill()`/`child.wait()` lifecycle in `Vm::stop`
/// keeps working unchanged.
///
/// Deliberately omits `--netns`: the current network model attaches tap
/// devices from a shared pool directly in the daemon's own network
/// namespace (see `network.rs`), with no per-VM network namespace to join
/// — chroot/cgroup/uid isolation and network-namespace isolation are
/// independent axes, and adding the latter is real, separate follow-up
/// work, not something to fold in here.
pub fn build_jailer_args(
    launch: &JailLaunch,
    jail_instance_id: &str,
    firecracker_bin: &Path,
    cgroup_limits: &[String],
    api_sock_in_jail: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        OsString::from("--id"),
        OsString::from(jail_instance_id),
        OsString::from("--exec-file"),
        firecracker_bin.as_os_str().to_owned(),
        OsString::from("--uid"),
        OsString::from(launch.uid.to_string()),
        OsString::from("--gid"),
        OsString::from(launch.gid.to_string()),
        OsString::from("--chroot-base-dir"),
        launch.chroot_base_dir.as_os_str().to_owned(),
        OsString::from("--cgroup-version"),
        OsString::from("2"),
    ];
    for limit in cgroup_limits {
        args.push(OsString::from("--cgroup"));
        args.push(OsString::from(limit));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from("--api-sock"));
    args.push(api_sock_in_jail.as_os_str().to_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(test_name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("sandkiln-jailer-test-{test_name}-{}", std::process::id()));
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

    #[test]
    fn jail_instance_id_is_stable_and_readable() {
        assert_eq!(jail_instance_id(42), "sandkiln-42");
    }

    #[test]
    fn chroot_root_follows_firecracker_exec_name_then_id_then_root() {
        let root = chroot_root(Path::new("/srv/jailer"), Path::new("/usr/bin/firecracker"), "sandkiln-7");
        assert_eq!(root, PathBuf::from("/srv/jailer/firecracker/sandkiln-7/root"));
    }

    #[test]
    fn instance_dir_is_chroot_roots_parent() {
        let root = PathBuf::from("/srv/jailer/firecracker/sandkiln-7/root");
        assert_eq!(instance_dir(&root), PathBuf::from("/srv/jailer/firecracker/sandkiln-7"));
    }

    #[test]
    fn new_pool_has_exactly_the_configured_range() {
        let pool = JailerIdPool::new(600000..=600009);
        assert_eq!(pool.available(), 10);
    }

    #[test]
    fn lease_and_release_round_trip_without_growing_or_shrinking_the_pool() {
        let pool = JailerIdPool::new(600000..=600001);
        let a = pool.lease().unwrap();
        let b = pool.lease().unwrap();
        assert_ne!(a, b, "two concurrent leases must never return the same id");
        assert_eq!(pool.available(), 0);
        assert!(pool.lease().is_err(), "pool must be exhausted after leasing every id in the range");

        pool.release(a);
        assert_eq!(pool.available(), 1);
        let c = pool.lease().unwrap();
        assert_eq!(c, a, "a released id becomes available for lease again");
    }

    #[test]
    fn leased_ids_are_always_within_the_configured_range() {
        let pool = JailerIdPool::new(700000..=700004);
        let mut leased = Vec::new();
        while let Ok(id) = pool.lease() {
            assert!((700000..=700004).contains(&id));
            leased.push(id);
        }
        assert_eq!(leased.len(), 5);
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

        let linked = link_resource_into_jail(&source, &chroot_root, "kernel", false).unwrap();

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

        let linked = link_resource_into_jail(&source, &chroot_root, "rootfs.ext4", false).unwrap();
        let mode = fs::metadata(&linked.host_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o007, 0o004, "read-only resource must be world-readable, not world-writable");
    }

    #[test]
    fn link_resource_into_jail_makes_writable_resources_world_read_write() {
        let tmp = TempDir::new("link-writable");
        let source = tmp.path.join("rootfs-rw.ext4");
        fs::write(&source, b"rw").unwrap();
        let chroot_root = tmp.path.join("firecracker").join("sandkiln-4").join("root");

        let linked = link_resource_into_jail(&source, &chroot_root, "rootfs.ext4", true).unwrap();
        let mode = fs::metadata(&linked.host_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o007, 0o006, "writable resource (the rootfs) must be world read+write");
    }

    #[test]
    fn link_resource_into_jail_does_not_change_the_sources_own_permissions_destructively() {
        // Regression guard for the exact bug this design avoids: chowning
        // (rather than chmod-adding a permission bit) a hard link would
        // silently mutate the *source* file's ownership too, since a hard
        // link is the same inode. This test only asserts the source stays
        // readable/writable by its own owner afterward — a chown to an
        // unrelated uid would typically break that for the original owner
        // on a real multi-user system, even though a same-uid test process
        // wouldn't itself observe an ownership change directly.
        let tmp = TempDir::new("link-source-untouched");
        let source = tmp.path.join("shared-kernel");
        fs::write(&source, b"shared").unwrap();
        let original_mode = fs::metadata(&source).unwrap().permissions().mode();

        let chroot_root = tmp.path.join("firecracker").join("sandkiln-5").join("root");
        link_resource_into_jail(&source, &chroot_root, "kernel", false).unwrap();

        // The source's own mode bits are only ever widened (adding
        // other-read), never replaced — the owner/group bits it had
        // before linking are untouched.
        let after_mode = fs::metadata(&source).unwrap().permissions().mode();
        assert_eq!(after_mode & 0o700, original_mode & 0o700, "owner permission bits must be unchanged");
    }

    #[test]
    fn cgroup_memory_max_bytes_adds_vmm_overhead_margin() {
        let bytes = cgroup_memory_max_bytes(512);
        assert_eq!(bytes, (512 + 128) * 1024 * 1024);
        assert!(bytes > 512 * 1024 * 1024, "ceiling must exceed the guest's own configured RAM");
    }

    #[test]
    fn cgroup_cpu_max_scales_quota_linearly_with_vcpu_count() {
        assert_eq!(cgroup_cpu_max(1), "100000 100000");
        assert_eq!(cgroup_cpu_max(2), "200000 100000");
        assert_eq!(cgroup_cpu_max(4), "400000 100000");
    }

    #[test]
    fn cgroup_limits_includes_both_memory_and_cpu_controllers() {
        let limits = cgroup_limits(512, 2);
        assert_eq!(limits.len(), 2);
        assert!(limits[0].starts_with("memory.max="));
        assert!(limits[1].starts_with("cpu.max="));
    }

    #[test]
    fn build_jailer_args_places_the_separator_before_firecrackers_own_args() {
        let launch = JailLaunch {
            jailer_bin: PathBuf::from("/usr/bin/jailer"),
            chroot_base_dir: PathBuf::from("/srv/jailer"),
            uid: 600001,
            gid: 600001,
        };
        let args = build_jailer_args(
            &launch,
            "sandkiln-9",
            Path::new("/usr/bin/firecracker"),
            &["memory.max=1073741824".to_string()],
            Path::new("/api.sock"),
        );

        let separator_index = args.iter().position(|a| a == "--").expect("must contain a -- separator");
        let after: Vec<&OsString> = args[separator_index + 1..].iter().collect();
        assert_eq!(after, vec![&OsString::from("--api-sock"), &OsString::from("/api.sock")]);

        let before: Vec<String> = args[..separator_index].iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(before.contains(&"sandkiln-9".to_string()));
        assert!(before.contains(&"600001".to_string()));
        assert!(before.contains(&"memory.max=1073741824".to_string()));
        assert!(before.contains(&"2".to_string()), "cgroup-version 2 must always be requested");
    }

    #[test]
    fn build_jailer_args_omits_daemonize_and_netns() {
        let launch = JailLaunch { jailer_bin: PathBuf::from("/usr/bin/jailer"), chroot_base_dir: PathBuf::from("/srv/jailer"), uid: 1, gid: 1 };
        let args = build_jailer_args(&launch, "sandkiln-1", Path::new("/usr/bin/firecracker"), &[], Path::new("/api.sock"));
        let joined: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(!joined.contains(&"--daemonize".to_string()));
        assert!(!joined.contains(&"--netns".to_string()));
    }
}

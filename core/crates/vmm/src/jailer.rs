//! Firecracker's jailer: re-execs `firecracker` inside a chroot'd,
//! cgroup-limited environment under a dedicated unprivileged uid/gid,
//! instead of `vm::boot`'s default direct spawn. See
//! `docs/design-notes/security-hardening.md` and the jailer setup in
//! `docs-site/self-hosting/manual-setup.mdx`.
//!
//! **Why the daemon can't do this itself**: chroot(2), setuid/setgid,
//! `/dev/kvm`/`/dev/net/tun` device nodes, and cgroup management all need
//! privileges the daemon deliberately doesn't have (ambient
//! `CAP_NET_ADMIN` only — see root `AGENTS.md`). Jailer holds those
//! privileges only for the brief setup window, then drops them and execs
//! `firecracker` as the target uid/gid — the standard fix is making
//! `jailer` itself setuid-root, a small purpose-built binary, not the
//! whole daemon.
//!
//! **What crosses the chroot boundary**: once jailer calls `chroot()`,
//! firecracker can't see any host path outside its jail root. Every
//! referenced file (kernel, rootfs, drives) must exist inside the jail
//! *before* jailer starts, and every path in Firecracker's own API
//! afterward must be the in-jail path. `link_resource_into_jail` puts a
//! host file in the jail; `vm::boot_inner` rewrites API bodies to the
//! resulting in-jail paths.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::ops::RangeInclusive;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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
/// range, takes them back on release. Mirrors
/// `crate::network::NetworkManager`'s tap/IP pool — a bounded,
/// pre-declared id space rather than hashing a sandbox id into one,
/// because a collision here is a real security bug: two jailed VMs
/// sharing a uid means one guest's escaped process can
/// `kill`/`ptrace`/read the other's files.
///
/// Configure the range outside normal system/user uids (600000+,
/// mirroring `/etc/subuid` convention) so a jailed uid never collides
/// with a real host account. One number serves as both uid and gid —
/// simpler to reason about, no reason for a VM's group to be shared with
/// anything else.
pub struct JailerIdPool {
    free: Mutex<VecDeque<u32>>,
    quarantined: Mutex<Vec<u32>>,
}

impl JailerIdPool {
    pub fn new(range: RangeInclusive<u32>) -> Self {
        Self { free: Mutex::new(range.collect()), quarantined: Mutex::new(Vec::new()) }
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

    /// Withholds `id` from reuse for the rest of this process's life. For a
    /// uid whose process could not be killed: handing the uid to the next
    /// VM would let a live, possibly guest-controlled, process share an
    /// identity with it.
    pub fn quarantine(&self, id: u32) {
        self.quarantined.lock().unwrap().push(id);
    }

    /// Ids withheld by [`Self::quarantine`].
    pub fn quarantined(&self) -> Vec<u32> {
        self.quarantined.lock().unwrap().clone()
    }
}

/// A stable, jailer-`--id`-safe identifier for one VM, from the same
/// monotonic counter `vm::Vm` uses for socket paths — keeps every
/// jailer/chroot/socket path traceable to one number in the logs.
pub fn jail_instance_id(vm_id: u64) -> String {
    format!("sandkiln-{vm_id}")
}

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
/// Made world-readable (world-writable too for `writable` resources —
/// the rootfs) rather than `chown`ed to the jail's uid/gid: a hard link
/// shares one inode with the source, so `chown` would silently change the
/// *source* file's ownership too (a shared kernel image, another
/// sandbox's drive). Permission bits scoped to "other" avoid that.
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

/// cgroup v2 memory ceiling: guest RAM plus headroom for Firecracker's
/// own VMM overhead (page tables, virtio buffers, vsock/balloon
/// backends). Without this margin a VM sized right at its cgroup ceiling
/// gets OOM-killed before the guest finishes booting — the ceiling bounds
/// the whole jailed process, not just guest-visible RAM.
pub fn cgroup_memory_max_bytes(mem_size_mib: u32) -> u64 {
    const VMM_OVERHEAD_MIB: u64 = 128;
    (mem_size_mib as u64 + VMM_OVERHEAD_MIB) * 1024 * 1024
}

/// cgroup v2's `cpu.max` (`"<quota> <period>"` in microseconds), pinning
/// one jailed VM to at most `vcpu_count` fully-utilized cores — without it
/// a runaway guest can starve other sandboxes' vCPU threads, which are
/// ordinary host threads with no isolation of their own beyond this.
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

/// Builds the full jailer argv (excluding argv[0]) for one VM. Pure and
/// testable — `vm::boot_inner`/`vm::resume` pass this straight to
/// `Command::args`.
///
/// Omits `--daemonize`: jailer stays attached to its parent and `exec`s
/// firecracker in place rather than forking/detaching, so the `Child`
/// handle `Command::spawn` returns *becomes* the firecracker process
/// after exec (same pid) — `Vm::stop`'s existing `kill()`/`wait()`
/// lifecycle keeps working unchanged.
///
/// Omits `--netns`: tap devices attach from a shared pool directly in the
/// daemon's own network namespace (`network.rs`), no per-VM namespace to
/// join. Network-namespace isolation is a separate, not-yet-built axis
/// from chroot/cgroup/uid isolation.
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

/// Jail instance id of the startup self-check's throwaway jail.
const SELF_CHECK_INSTANCE: &str = "sandkiln-selfcheck";

/// How long the self-check waits for its throwaway jailer run to finish.
const SELF_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether `meta` describes a setuid-root file: the way the unprivileged
/// daemon is allowed to run `jailer` (chroot, setuid, cgroups, device nodes
/// all need root).
pub fn is_setuid_root(meta: &fs::Metadata) -> bool {
    meta.mode() & 0o4000 != 0 && meta.uid() == 0
}

/// Turns the result of "can the daemon list the jail root" into the
/// startup verdict. Pure so the wording and the classification are tested
/// without a real jailer. A permission error is the known failure of
/// finding 32 in `docs/findings.md`; anything else is reported as is.
fn judge_jail_access(access: io::Result<()>, root: &Path, uid: u32) -> Result<(), String> {
    match access {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(format!(
            "the daemon cannot enter the jail root {} that jailer just created (owned by uid {uid}, mode 0700). \
             The unprivileged daemon therefore could not reach a jailed Firecracker's API socket, and every jailed create would fail \
             (finding 32 in docs/findings.md). Turn jailer_enabled off, or see docs-site/self-hosting/jailer.mdx",
            root.display()
        )),
        Err(e) => Err(format!("cannot inspect the jail root {}: {e}", root.display())),
    }
}

/// Startup probe for `jailer_enabled`: runs the real `jailer` once against a
/// throwaway jail that runs `firecracker --version` (a static binary, so it runs inside an empty chroot), then checks, as the daemon's own user,
/// that it can enter the jail root afterwards. That is the exact access a
/// real boot needs to reach Firecracker's API socket. It also proves, with
/// one run, the other prerequisites: `jailer` is setuid root, cgroup v2 is
/// usable, the uid is accepted, the chroot base is writable.
///
/// Why a probe and not a static check: whether the daemon can enter the
/// jail depends on what jailer does to ownership and modes, which is only
/// observable by running it as root, and the daemon is deliberately not.
/// The one thing a probe cannot do is clean up after a failed run: jailer's
/// files are owned by `probe_uid`, so on the known-bad configuration the
/// throwaway directory stays (one fixed name, reused by the next probe) and
/// the error names it.
pub fn self_check(launch: &JailLaunch, firecracker_bin: &Path) -> Result<(), String> {
    let meta = fs::metadata(&launch.jailer_bin).map_err(|e| format!("cannot stat jailer binary {}: {e}", launch.jailer_bin.display()))?;
    if !is_setuid_root(&meta) {
        return Err(format!(
            "{} is not setuid root; jailer needs root for chroot, setuid and cgroups. Run: sudo chown root:root {0} && sudo chmod u+s {0}",
            launch.jailer_bin.display()
        ));
    }
    run_probe(launch, firecracker_bin)
}

/// The probe itself, past the setuid gate: one jailer run, then the access
/// test. Separate from [`self_check`] so tests can drive it with a stand-in
/// jailer that is not setuid root.
fn run_probe(launch: &JailLaunch, firecracker_bin: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let root = chroot_root(&launch.chroot_base_dir, firecracker_bin, SELF_CHECK_INSTANCE);
    let instance = instance_dir(&root);
    let _ = fs::remove_dir_all(&instance);
    prepare_chroot_dir(&root).map_err(|e| format!("cannot create the probe jail under {}: {e}", launch.chroot_base_dir.display()))?;

    // Same argv as a real boot (cgroup limits included: jailer cannot move itself
    // into the shared parent cgroup without them) up to the separator, then a
    // no-op for Firecracker.
    let mut args = build_jailer_args(launch, SELF_CHECK_INSTANCE, firecracker_bin, &cgroup_limits(128, 1), Path::new("/api.sock"));
    let separator = args.iter().position(|a| a == "--").expect("build_jailer_args always emits `--`");
    args.truncate(separator + 1);
    args.push(OsString::from("--version"));
    let mut child = Command::new(&launch.jailer_bin)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", launch.jailer_bin.display()))?;

    let deadline = Instant::now() + SELF_CHECK_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                return Err(format!("the jailer probe did not finish within {SELF_CHECK_TIMEOUT:?}"));
            }
            Err(e) => return Err(format!("waiting for the jailer probe failed: {e}")),
        }
    };
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = io::Read::read_to_string(&mut pipe, &mut stderr);
        }
        return Err(format!("the jailer probe failed ({status}): {}", stderr.trim()));
    }

    let verdict = judge_jail_access(fs::read_dir(&root).map(|_| ()), &root, launch.uid);
    match &verdict {
        Ok(()) => {
            let _ = fs::remove_dir_all(&instance);
        }
        Err(_) if fs::remove_dir_all(&instance).is_err() => {
            return verdict.map_err(|e| format!("{e}. The probe left {} behind (root-owned files inside); remove it with sudo", instance.display()));
        }
        Err(_) => {}
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Regression guard: chowning a hard link (instead of chmod-adding
        // a bit) would silently mutate the *source* file's ownership too,
        // since they share one inode.
        let tmp = TempDir::new("link-source-untouched");
        let source = tmp.path.join("shared-kernel");
        fs::write(&source, b"shared").unwrap();
        let original_mode = fs::metadata(&source).unwrap().permissions().mode();

        let chroot_root = tmp.path.join("firecracker").join("sandkiln-5").join("root");
        link_resource_into_jail(&source, &chroot_root, "kernel", false).unwrap();

        // Mode bits are only ever widened (adding other-read), never replaced.
        let after_mode = fs::metadata(&source).unwrap().permissions().mode();
        assert_eq!(after_mode & 0o700, original_mode & 0o700, "owner permission bits must be unchanged");
    }

    #[test]
    fn quarantined_ids_never_return_to_the_pool() {
        let pool = JailerIdPool::new(600000..=600001);
        let a = pool.lease().unwrap();
        pool.quarantine(a);
        assert_eq!(pool.quarantined(), vec![a]);
        let b = pool.lease().unwrap();
        assert_ne!(a, b);
        assert!(pool.lease().is_err(), "the quarantined id must stay out of circulation");
    }

    #[test]
    fn setuid_root_needs_both_the_bit_and_root_ownership() {
        let tmp = TempDir::new("setuid-root");
        let file = tmp.path.join("jailer");
        fs::write(&file, b"x").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o4755)).unwrap();
        // Owned by the test user, not root: the bit alone is not enough.
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(is_setuid_root(&meta), meta.uid() == 0);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!is_setuid_root(&fs::metadata(&file).unwrap()));
    }

    #[test]
    fn judge_jail_access_names_finding_32_on_permission_denied() {
        let verdict = judge_jail_access(Err(io::Error::from(io::ErrorKind::PermissionDenied)), Path::new("/j/root"), 600001);
        let msg = verdict.unwrap_err();
        assert!(msg.contains("finding 32") && msg.contains("/j/root") && msg.contains("600001"), "{msg}");
    }

    #[test]
    fn judge_jail_access_passes_through_other_errors_and_success() {
        assert!(judge_jail_access(Ok(()), Path::new("/j/root"), 1).is_ok());
        let msg = judge_jail_access(Err(io::Error::from(io::ErrorKind::NotFound)), Path::new("/j/root"), 1).unwrap_err();
        assert!(!msg.contains("finding 32"), "{msg}");
    }

    /// A stand-in `jailer` shell script: parses `--id`/`--chroot-base-dir`/
    /// `--exec-file` like the real one, creates the jail root, then applies
    /// `root_mode` to it. Not setuid root, so `self_check`'s first gate is
    /// bypassed by calling `run_probe` directly.
    fn fake_jailer(dir: &Path, root_mode: &str, exit_code: i32) -> PathBuf {
        let script = dir.join("fake-jailer");
        let body = format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case $1 in --id) id=$2; shift;; --chroot-base-dir) base=$2; shift;; --exec-file) exe=$2; shift;; esac; shift; done\n\
             root=\"$base/$(basename $exe)/$id/root\"; mkdir -p \"$root\"; chmod {root_mode} \"$root\"; echo probe-stderr >&2; exit {exit_code}\n"
        );
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn probe_launch(tmp: &TempDir, jailer: PathBuf) -> JailLaunch {
        JailLaunch { jailer_bin: jailer, chroot_base_dir: tmp.path.join("jail"), uid: 600000, gid: 600000 }
    }

    #[test]
    fn self_check_rejects_a_jailer_that_is_not_setuid_root() {
        let tmp = TempDir::new("selfcheck-not-setuid");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "700", 0));
        let err = self_check(&launch, Path::new("/usr/bin/firecracker")).unwrap_err();
        assert!(err.contains("not setuid root"), "{err}");
    }

    #[test]
    fn self_check_reports_a_missing_jailer_binary() {
        let tmp = TempDir::new("selfcheck-missing");
        let launch = probe_launch(&tmp, tmp.path.join("no-such-jailer"));
        assert!(self_check(&launch, Path::new("/usr/bin/firecracker")).unwrap_err().contains("cannot stat"));
    }

    #[test]
    fn probe_reports_the_jailers_own_failure_output() {
        let tmp = TempDir::new("probe-jailer-fails");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "755", 3));
        let msg = run_probe(&launch, Path::new("/usr/bin/firecracker")).unwrap_err();
        assert!(msg.contains("probe failed") && msg.contains("probe-stderr"), "{msg}");
    }

    #[test]
    fn probe_passes_when_the_daemon_can_enter_the_jail_root() {
        let tmp = TempDir::new("probe-ok");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "755", 0));
        assert!(run_probe(&launch, Path::new("/usr/bin/firecracker")).is_ok());
    }

    #[test]
    fn probe_detects_a_jail_root_the_daemon_cannot_enter() {
        // As root every directory is enterable, so the condition cannot be built.
        if std::process::Command::new("id").arg("-u").output().map(|o| o.stdout == b"0\n").unwrap_or(false) {
            return;
        }
        let tmp = TempDir::new("probe-bad");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "000", 0));
        let msg = run_probe(&launch, Path::new("/usr/bin/firecracker")).unwrap_err();
        assert!(msg.contains("finding 32"), "{msg}");
        // Let TempDir's drop remove what the stand-in locked.
        let root = chroot_root(&launch.chroot_base_dir, Path::new("/usr/bin/firecracker"), SELF_CHECK_INSTANCE);
        fs::set_permissions(root, fs::Permissions::from_mode(0o755)).unwrap();
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

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
//! **Who owns what, and how the daemon reaches the jail.** Jailer runs the
//! `--exec-file` as the per-VM uid (distinct per concurrent VM) and the shared
//! jail group (`JailLaunch::gid`), which the daemon's user belongs to. The
//! exec file is not Firecracker but `sandkiln-firecracker-supervisor`
//! (`crates/jail-supervisor`): jailer makes the chroot root mode 0700, and
//! the supervisor, which owns it, widens it to the jail group before running
//! Firecracker, so the daemon can reach the API and vsock sockets and remove
//! the jail afterwards. The supervisor also ends Firecracker when the pipe
//! the daemon holds to its stdin closes, because the daemon may not signal a
//! process of another uid. See `docs/design-notes/security-hardening.md` and
//! finding 32 in `docs/findings.md` for the options weighed.
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
/// `uid` must come from a [`JailerIdPool`] (or an equivalent allocation the
/// caller guarantees is unique among concurrently running jailed VMs);
/// nothing here re-validates that.
#[derive(Clone)]
pub struct JailLaunch {
    pub jailer_bin: PathBuf,
    /// Runs in the chroot in place of `firecracker`; see the module docs.
    /// Its file name must contain "firecracker" (jailer insists).
    pub supervisor_bin: PathBuf,
    pub chroot_base_dir: PathBuf,
    /// Per-VM: a distinct uid for every concurrently running jailed VM.
    pub uid: u32,
    /// Shared by every jailed VM, and the daemon's user must be a member.
    /// It is what gives the daemon access to the jail and to the tap pool
    /// (`scripts/host-setup/prepare-jailer.sh`).
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
/// with a real host account. Only the uid is per VM; the gid is one shared
/// jail group (`JailLaunch::gid`), so the isolation between two sandboxes
/// rests on distinct uids and distinct chroots, not on distinct groups.
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

/// Where jailer's cgroup v2 hierarchy lives: `<root>/<exec file name>/<id>`.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Where jailer puts one VM's cgroup. Jailer never removes it, so every
/// jailed boot would otherwise leak one for the life of the host.
pub fn cgroup_dir(supervisor_bin: &Path, jail_instance_id: &str) -> PathBuf {
    let exec_name = supervisor_bin.file_name().expect("supervisor_bin must have a file name");
    Path::new(CGROUP_ROOT).join(exec_name).join(jail_instance_id)
}

/// Removes a finished VM's cgroup. Works for the unprivileged daemon only
/// when it owns the parent cgroup directory (`prepare-jailer.sh` arranges
/// that): removing a cgroup needs write access to its parent, not to the
/// cgroup itself. An already-missing cgroup is success.
pub fn remove_cgroup(supervisor_bin: &Path, jail_instance_id: &str) -> io::Result<()> {
    match fs::remove_dir(cgroup_dir(supervisor_bin, jail_instance_id)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// What one startup sweep found.
#[derive(Debug, Default, PartialEq)]
pub struct SweepReport {
    pub removed: Vec<String>,
    /// Jails that could not be removed, with the reason.
    pub failed: Vec<(String, String)>,
}

/// Removes jails (and their cgroups) a previous daemon process left behind.
/// A jailed VM cannot outlive its daemon (the supervisor ends Firecracker
/// when the daemon's pipe closes), so at startup every `sandkiln-*` jail is
/// stale. It also matters for correctness: jail ids restart from 1 with each
/// process, so a leftover `sandkiln-1` would collide with the next boot.
pub fn sweep_stale_jails(chroot_base_dir: &Path, supervisor_bin: &Path) -> SweepReport {
    sweep_stale_jails_in(chroot_base_dir, supervisor_bin, Path::new(CGROUP_ROOT))
}

fn sweep_stale_jails_in(chroot_base_dir: &Path, supervisor_bin: &Path, cgroup_root: &Path) -> SweepReport {
    let mut report = SweepReport::default();
    let exec_name = supervisor_bin.file_name().expect("supervisor_bin must have a file name");
    let jails = chroot_base_dir.join(exec_name);
    let Ok(entries) = fs::read_dir(&jails) else { return report };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("sandkiln-") {
            continue;
        }
        match fs::remove_dir_all(entry.path()) {
            Ok(()) => report.removed.push(name.clone()),
            Err(e) => report.failed.push((name.clone(), e.to_string())),
        }
        // A missing cgroup is the normal case; a busy one means a process
        // still runs in it, which is reported by the failed jail removal.
        let _ = fs::remove_dir(cgroup_root.join(exec_name).join(&name));
    }
    report
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
    cgroup_limits: &[String],
    api_sock_in_jail: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        OsString::from("--id"),
        OsString::from(jail_instance_id),
        OsString::from("--exec-file"),
        launch.supervisor_bin.as_os_str().to_owned(),
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

/// Turns the result of "can the daemon use the jail root" into the startup
/// verdict. Pure so the wording and the classification are tested without a
/// real jailer. A permission error means the supervisor's widening did not
/// reach the daemon: the finding 32 failure (`docs/findings.md`).
fn judge_jail_access(access: io::Result<()>, root: &Path, gid: u32) -> Result<(), String> {
    match access {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(format!(
            "the daemon cannot use the jail root {} (finding 32 in docs/findings.md). Jailer makes it mode 0700 and the jail supervisor \
             opens it to group {gid}, so the daemon's user must be a member of group {gid} in the session that started it \
             (check `id`; a group added after login is not picked up). Run scripts/host-setup/prepare-jailer.sh, see \
             docs-site/self-hosting/jailer.mdx. Underlying error: {e}",
            root.display()
        )),
        Err(e) => Err(format!("cannot use the jail root {}: {e}", root.display())),
    }
}

/// What the daemon must be able to do in a jail after the supervisor ran:
/// enter the root, create and remove an entry in it (teardown deletes the
/// whole tree the same way).
fn exercise_jail(root: &Path) -> io::Result<()> {
    fs::read_dir(root)?;
    let probe = root.join(".sandkiln-probe");
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)
}

/// Startup probe for `jailer_enabled`: runs the real `jailer` once against a
/// throwaway jail whose supervisor exits immediately (`--self-check`), then
/// checks, as the daemon's own user, that it can use the jail root, tear the
/// jail down and remove the cgroup. Those are exactly the accesses a real
/// boot depends on. One run also proves the other prerequisites: `jailer` is
/// setuid root, cgroup v2 is usable, the uid is accepted, the chroot base is
/// writable, the supervisor is a static binary that runs in an empty chroot.
///
/// Why a probe and not a static check: what the daemon can do in the jail
/// depends on what jailer does to ownership and modes and on the daemon's
/// group membership, which is only observable by running it. A failed probe
/// can leave root-owned files only root can remove (one fixed jail name,
/// reused by the next probe); the error names it.
pub fn self_check(launch: &JailLaunch) -> Result<(), String> {
    let meta = fs::metadata(&launch.jailer_bin).map_err(|e| format!("cannot stat jailer binary {}: {e}", launch.jailer_bin.display()))?;
    if !is_setuid_root(&meta) {
        return Err(format!(
            "{} is not setuid root; jailer needs root for chroot, setuid and cgroups. Run: sudo chown root:root {0} && sudo chmod u+s {0}",
            launch.jailer_bin.display()
        ));
    }
    if !launch.supervisor_bin.is_file() {
        return Err(format!(
            "the jail supervisor {} does not exist; build it with `cargo build --release -p sandkiln-jail-supervisor --target x86_64-unknown-linux-musl` and install it (scripts/host-setup/prepare-jailer.sh)",
            launch.supervisor_bin.display()
        ));
    }
    run_probe(launch)
}

/// The probe itself, past the static gates: one jailer run, then the access
/// tests. Separate from [`self_check`] so tests can drive it with a stand-in
/// jailer that is not setuid root.
fn run_probe(launch: &JailLaunch) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let root = chroot_root(&launch.chroot_base_dir, &launch.supervisor_bin, SELF_CHECK_INSTANCE);
    let instance = instance_dir(&root);
    let _ = fs::remove_dir_all(&instance);
    let _ = remove_cgroup(&launch.supervisor_bin, SELF_CHECK_INSTANCE);
    prepare_chroot_dir(&root).map_err(|e| format!("cannot create the probe jail under {}: {e}", launch.chroot_base_dir.display()))?;

    // Same argv as a real boot (cgroup limits included: jailer cannot move itself
    // into the shared parent cgroup without them) up to the separator, then the
    // supervisor's own no-op mode.
    let mut args = build_jailer_args(launch, SELF_CHECK_INSTANCE, &cgroup_limits(128, 1), Path::new("/api.sock"));
    let separator = args.iter().position(|a| a == "--").expect("build_jailer_args always emits `--`");
    args.truncate(separator + 1);
    args.push(OsString::from("--self-check"));
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

    let verdict = judge_jail_access(exercise_jail(&root), &root, launch.gid);
    if verdict.is_err() {
        return verdict.map_err(|e| match fs::remove_dir_all(&instance) {
            Ok(()) => e,
            Err(_) => format!("{e}. The probe left {} behind; remove it with sudo", instance.display()),
        });
    }
    fs::remove_dir_all(&instance).map_err(|e| format!("cannot remove the probe jail {}: {e}", instance.display()))?;
    remove_cgroup(&launch.supervisor_bin, SELF_CHECK_INSTANCE).map_err(|e| {
        format!(
            "cannot remove the probe's cgroup {}: {e}. The daemon must own the parent cgroup directory so it can remove each VM's cgroup (scripts/host-setup/prepare-jailer.sh)",
            cgroup_dir(&launch.supervisor_bin, SELF_CHECK_INSTANCE).display()
        )
    })
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

        let linked = link_resource_into_jail(&source, &chroot_root, "kernel", false, own_gid(&source)).unwrap();

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

        let linked = link_resource_into_jail(&source, &chroot_root, "rootfs.ext4", false, own_gid(&source)).unwrap();
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

        let linked = link_resource_into_jail(&source, &chroot_root, "rootfs.ext4", true, own_gid(&source)).unwrap();
        let mode = fs::metadata(&linked.host_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o060, 0o060, "writable resource (the rootfs) must be group read+write");
        assert_eq!(mode & 0o002, 0, "and never world-writable");
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
        link_resource_into_jail(&source, &chroot_root, "kernel", false, own_gid(&source)).unwrap();

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
        assert!(msg.contains("finding 32") && msg.contains("/j/root") && msg.contains("group 600001"), "{msg}");
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
        JailLaunch {
            jailer_bin: jailer,
            supervisor_bin: PathBuf::from("/usr/bin/sandkiln-firecracker-supervisor"),
            chroot_base_dir: tmp.path.join("jail"),
            uid: 600000,
            gid: 600000,
        }
    }

    #[test]
    fn self_check_rejects_a_jailer_that_is_not_setuid_root() {
        let tmp = TempDir::new("selfcheck-not-setuid");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "700", 0));
        let err = self_check(&launch).unwrap_err();
        assert!(err.contains("not setuid root"), "{err}");
    }

    #[test]
    fn self_check_reports_a_missing_jailer_binary() {
        let tmp = TempDir::new("selfcheck-missing");
        let launch = probe_launch(&tmp, tmp.path.join("no-such-jailer"));
        assert!(self_check(&launch).unwrap_err().contains("cannot stat"));
    }

    #[test]
    fn probe_reports_the_jailers_own_failure_output() {
        let tmp = TempDir::new("probe-jailer-fails");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "755", 3));
        let msg = run_probe(&launch).unwrap_err();
        assert!(msg.contains("probe failed") && msg.contains("probe-stderr"), "{msg}");
    }

    #[test]
    fn probe_passes_when_the_daemon_can_enter_the_jail_root() {
        let tmp = TempDir::new("probe-ok");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "755", 0));
        assert!(run_probe(&launch).is_ok());
    }

    #[test]
    fn probe_detects_a_jail_root_the_daemon_cannot_enter() {
        // As root every directory is enterable, so the condition cannot be built.
        if std::process::Command::new("id").arg("-u").output().map(|o| o.stdout == b"0\n").unwrap_or(false) {
            return;
        }
        let tmp = TempDir::new("probe-bad");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "000", 0));
        let msg = run_probe(&launch).unwrap_err();
        assert!(msg.contains("finding 32"), "{msg}");
        // Let TempDir's drop remove what the stand-in locked.
        let root = chroot_root(&launch.chroot_base_dir, &launch.supervisor_bin, SELF_CHECK_INSTANCE);
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

    fn launch(uid: u32, gid: u32) -> JailLaunch {
        JailLaunch {
            jailer_bin: PathBuf::from("/usr/bin/jailer"),
            supervisor_bin: PathBuf::from("/opt/sandkiln-firecracker-supervisor"),
            chroot_base_dir: PathBuf::from("/srv/jailer"),
            uid,
            gid,
        }
    }

    #[test]
    fn build_jailer_args_places_the_separator_before_the_supervisors_own_args() {
        let args = build_jailer_args(&launch(600001, 599000), "sandkiln-9", &["memory.max=1073741824".to_string()], Path::new("/api.sock"));

        let separator_index = args.iter().position(|a| a == "--").expect("must contain a -- separator");
        let after: Vec<&OsString> = args[separator_index + 1..].iter().collect();
        assert_eq!(after, vec![&OsString::from("--api-sock"), &OsString::from("/api.sock")]);

        let before: Vec<String> = args[..separator_index].iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(before.contains(&"sandkiln-9".to_string()));
        assert!(before.contains(&"memory.max=1073741824".to_string()));
        assert!(before.contains(&"2".to_string()), "cgroup-version 2 must always be requested");
    }

    #[test]
    fn build_jailer_args_runs_the_supervisor_under_the_vms_uid_and_the_shared_gid() {
        let args: Vec<String> = build_jailer_args(&launch(600001, 599000), "x", &[], Path::new("/api.sock"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let value_after = |flag: &str| args[args.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!(value_after("--exec-file"), "/opt/sandkiln-firecracker-supervisor");
        assert_eq!(value_after("--uid"), "600001");
        assert_eq!(value_after("--gid"), "599000");
    }

    #[test]
    fn build_jailer_args_omits_daemonize_and_netns() {
        let args = build_jailer_args(&launch(1, 1), "sandkiln-1", &[], Path::new("/api.sock"));
        let joined: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(!joined.contains(&"--daemonize".to_string()));
        assert!(!joined.contains(&"--netns".to_string()));
    }

    #[test]
    fn cgroup_dir_follows_the_exec_file_name_then_the_instance_id() {
        assert_eq!(
            cgroup_dir(Path::new("/opt/sandkiln-firecracker-supervisor"), "sandkiln-3"),
            PathBuf::from("/sys/fs/cgroup/sandkiln-firecracker-supervisor/sandkiln-3")
        );
    }

    #[test]
    fn sweep_removes_only_sandkiln_jails_and_their_cgroups() {
        let tmp = TempDir::new("sweep");
        let supervisor = Path::new("/opt/sandkiln-firecracker-supervisor");
        let jails = tmp.path.join("jail").join("sandkiln-firecracker-supervisor");
        fs::create_dir_all(jails.join("sandkiln-1").join("root")).unwrap();
        fs::create_dir_all(jails.join("sandkiln-2")).unwrap();
        fs::create_dir_all(jails.join("someone-elses")).unwrap();
        let cgroups = tmp.path.join("cg");
        fs::create_dir_all(cgroups.join("sandkiln-firecracker-supervisor").join("sandkiln-1")).unwrap();

        let mut report = sweep_stale_jails_in(&tmp.path.join("jail"), supervisor, &cgroups);
        report.removed.sort();

        assert_eq!(report.removed, vec!["sandkiln-1".to_string(), "sandkiln-2".to_string()]);
        assert!(report.failed.is_empty());
        assert!(jails.join("someone-elses").exists(), "a directory that is not ours must be left alone");
        assert!(!jails.join("sandkiln-1").exists());
        assert!(!cgroups.join("sandkiln-firecracker-supervisor").join("sandkiln-1").exists());
    }

    #[test]
    fn sweep_with_no_jail_directory_is_a_no_op() {
        let tmp = TempDir::new("sweep-empty");
        let report = sweep_stale_jails_in(&tmp.path, Path::new("/opt/sandkiln-firecracker-supervisor"), &tmp.path);
        assert_eq!(report, SweepReport::default());
    }

    #[test]
    fn removing_a_cgroup_that_is_already_gone_is_success() {
        assert!(remove_cgroup(Path::new("/opt/no-such-supervisor-for-test"), "sandkiln-1").is_ok());
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

//! A single Firecracker microVM's lifecycle: boot, talk to its guest
//! agent, tear down. This is the Rust equivalent of what
//! `scripts/boot-test-vm.sh` does by hand — the daemon drives this
//! directly instead of shelling out.
//!
//! Snapshot/resume (`pause`, `snapshot`, `resume`, `ResumeConfig`) lives in
//! [`snapshot`] — split out because it's a distinct capability with its
//! own long gotchas, not because `Vm` itself is two structs.

mod snapshot;

use crate::firecracker_api::ApiClient;
use crate::jailer::{self, JailLaunch};
use crate::vsock_client;
use sandkiln_protocol::{Request, Response, AGENT_PORT};
use serde_json::json;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

pub use snapshot::ResumeConfig;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct NetworkConfig {
    pub tap_device: String,
    pub guest_ip: Ipv4Addr,
    pub gateway_ip: Ipv4Addr,
    pub guest_mac: String,
}

/// A non-root drive to attach at boot, in addition to the mandatory
/// rootfs — e.g. a persistent drive from `crate::drive::DriveStore`. Each
/// one becomes its own `PUT /drives/<drive_id>` call before
/// `InstanceStart`, and shows up inside the guest as a separate block
/// device (`/dev/vdb`, `/dev/vdc`, ... in attachment order).
#[derive(Clone)]
pub struct DriveConfig {
    /// Must be unique among all drives attached to this VM, and must not
    /// be `"rootfs"` (reserved for the root device).
    pub drive_id: String,
    pub path_on_host: PathBuf,
    pub read_only: bool,
}

pub struct VmConfig {
    pub firecracker_bin: PathBuf,
    pub kernel_path: PathBuf,
    /// Path to a rootfs image dedicated to this VM — the caller owns
    /// copy-on-boot semantics; this module writes to it in place.
    pub rootfs_path: PathBuf,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
    pub network: Option<NetworkConfig>,
    /// Additional (non-root) drives to attach at boot, e.g. persistent
    /// drives requested by the caller. Empty for a VM with just a rootfs.
    pub extra_drives: Vec<DriveConfig>,
    /// When set, boots via Firecracker's jailer (chroot, cgroup v2
    /// limits, a dedicated unprivileged uid/gid) instead of the direct
    /// process spawn used when this is `None`. See `crate::jailer`.
    pub jail: Option<JailLaunch>,
}

pub struct Vm {
    id: u64,
    child: Child,
    api_socket: PathBuf,
    vsock_socket: PathBuf,
    /// Set only for a jailed boot — the `<chroot_base>/<exec>/<id>`
    /// directory jailer owns for this VM. Removed wholesale on `stop()`;
    /// `None` for a direct (unjailed) boot, which has no such directory.
    jail_instance_dir: Option<PathBuf>,
}

impl Vm {
    /// Boots a new microVM. On failure, the returned error's message
    /// includes the path to this VM's captured guest console log (see
    /// [`console_log_path`]) — a guest kernel panic or agent crash before
    /// vsock comes up is otherwise completely invisible from the host, so
    /// pointing the caller at where to look is the least this can do.
    pub fn boot(config: &VmConfig) -> io::Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let log_path = console_log_path(id);
        Self::boot_inner(config, id, &log_path).map_err(|e| annotate_with_console_log(e, &log_path))
    }

    fn boot_inner(config: &VmConfig, id: u64, log_path: &Path) -> io::Result<Self> {
        let started = Instant::now();

        let mut target = match &config.jail {
            None => spawn_direct(config, id, log_path)?,
            Some(jail) => spawn_jailed(config, jail, id, log_path)?,
        };

        if let Err(e) = configure_and_start(config, &mut target) {
            // A boot that fails partway through the API PUT sequence
            // still has a live child process (jailer, or firecracker
            // directly) holding the console log fds and — for a jailed
            // boot — a real chroot directory with hard-linked copies of
            // the kernel/rootfs/drives. Leaving either behind is a
            // resource leak (an orphaned process for a direct boot) or a
            // real information-disclosure surface (a leftover
            // world-readable chroot for a jailed one), not just untidy
            // state — clean up exactly like `snapshot::resume` already
            // does for the equivalent failure.
            let _ = target.child.kill();
            let _ = target.child.wait();
            let _ = std::fs::remove_file(&target.api_socket);
            let _ = std::fs::remove_file(&target.vsock_socket);
            if let Some(dir) = &target.jail_instance_dir {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Err(e);
        }

        tracing::info!(
            vm_id = id,
            pid = target.child.id(),
            jailed = config.jail.is_some(),
            boot_ms = started.elapsed().as_millis(),
            "vm booted"
        );
        Ok(Self {
            id,
            child: target.child,
            api_socket: target.api_socket,
            vsock_socket: target.vsock_socket,
            jail_instance_dir: target.jail_instance_dir,
        })
    }

    /// Whether this VM is running under Firecracker's jailer (chroot,
    /// cgroup limits, dedicated uid/gid) rather than a direct process
    /// spawn. Used by callers that need to treat the two differently —
    /// e.g. snapshotting a jailed sandbox isn't supported yet (see
    /// `crate::jailer`'s module doc comment), so the daemon checks this
    /// before attempting one.
    pub fn is_jailed(&self) -> bool {
        self.jail_instance_dir.is_some()
    }

    /// Sends a request to the guest agent over vsock and waits for its
    /// response. The agent isn't guaranteed to be listening the instant
    /// InstanceStart returns — this retries briefly to absorb that.
    pub fn call(&self, request: &Request) -> io::Result<Response> {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(5);
        loop {
            match vsock_client::call(&self.vsock_socket, AGENT_PORT, request) {
                Ok(response) => {
                    tracing::debug!(vm_id = self.id, elapsed_ms = started.elapsed().as_millis(), "vsock call ok");
                    return Ok(response);
                }
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => {
                    tracing::warn!(vm_id = self.id, error = %e, "vsock call failed");
                    return Err(e);
                }
            }
        }
    }

    pub fn stop(mut self) -> io::Result<()> {
        // SIGKILL-ing Firecracker directly loses anything the guest
        // hasn't flushed from its page cache to the virtio-blk backing
        // file yet — this was silently losing recent writes to attached
        // drives (rootfs copies don't care, they're discarded anyway).
        // Best-effort: if the agent isn't reachable for any reason, fall
        // through to the kill rather than hang shutdown on it.
        if let Err(e) = self.call(&Request::Exec { command: "sync".to_string(), args: vec![] }) {
            tracing::warn!(vm_id = self.id, error = %e, "sync before stop failed, proceeding anyway");
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.api_socket);
        let _ = std::fs::remove_file(&self.vsock_socket);
        // Tears down everything jailer created for this VM in one shot —
        // the chroot (with its hard-linked kernel/rootfs/drive copies)
        // and anything else jailer keeps alongside it. A jailed VM's
        // `api_socket`/`vsock_socket` already live inside this directory,
        // so the two removals above are redundant with this one but kept
        // for a direct (unjailed) boot, where this is `None`.
        if let Some(dir) = &self.jail_instance_dir {
            if let Err(e) = std::fs::remove_dir_all(dir) {
                tracing::warn!(vm_id = self.id, error = %e, dir = %dir.display(), "failed to remove jail instance directory");
            }
        }
        tracing::info!(vm_id = self.id, jailed = self.jail_instance_dir.is_some(), "vm stopped");
        Ok(())
    }
}

/// Everything `boot_inner` needs after spawning the child process
/// (direct or jailed), gathered in one place so the API-configuration
/// sequence that follows doesn't need to branch on jail-vs-direct at all —
/// only the paths differ, not the sequence of calls.
struct BootTarget {
    child: Child,
    /// Host-visible path to Firecracker's API socket — always a real
    /// host path, even for a jailed boot (`<chroot_root>/api.sock`),
    /// since the host process (this one) never enters the chroot itself.
    api_socket: PathBuf,
    /// Host-visible path to the vsock UDS, for `Vm::call`'s own
    /// connections after boot.
    vsock_socket: PathBuf,
    /// The value to send Firecracker's API for `kernel_image_path` — a
    /// plain host path for a direct boot, an in-jail path (e.g.
    /// `/kernel`) for a jailed one, since Firecracker itself can only see
    /// the latter once jailer has called `chroot()`.
    kernel_image_path: PathBuf,
    rootfs_path: PathBuf,
    /// Parallel to `VmConfig::extra_drives`, by index.
    drive_paths: Vec<PathBuf>,
    /// The value to send Firecracker's API for `/vsock`'s `uds_path` —
    /// same host/in-jail distinction as `kernel_image_path`.
    vsock_uds_path: PathBuf,
    jail_instance_dir: Option<PathBuf>,
}

fn spawn_direct(config: &VmConfig, id: u64, log_path: &Path) -> io::Result<BootTarget> {
    let api_socket = PathBuf::from(format!("/tmp/sandkiln-fc-{id}.sock"));
    let vsock_socket = PathBuf::from(format!("/tmp/sandkiln-vsock-{id}.sock"));
    let _ = std::fs::remove_file(&api_socket);
    let _ = std::fs::remove_file(&vsock_socket);

    let (stdout, stderr) = console_log_stdio(log_path)?;
    let child = Command::new(&config.firecracker_bin).arg("--api-sock").arg(&api_socket).stdout(stdout).stderr(stderr).spawn()?;

    Ok(BootTarget {
        child,
        api_socket,
        kernel_image_path: config.kernel_path.clone(),
        rootfs_path: config.rootfs_path.clone(),
        drive_paths: config.extra_drives.iter().map(|d| d.path_on_host.clone()).collect(),
        vsock_uds_path: vsock_socket.clone(),
        vsock_socket,
        jail_instance_dir: None,
    })
}

/// Spawns Firecracker via jailer instead of directly: builds the chroot,
/// links every resource the VM config references into it, then execs
/// jailer. If anything fails partway (a link fails, the spawn itself
/// fails), the partially-built chroot directory is removed — a half-built
/// jail with, say, only the kernel linked in is not a state worth leaving
/// on disk.
fn spawn_jailed(config: &VmConfig, jail: &JailLaunch, id: u64, log_path: &Path) -> io::Result<BootTarget> {
    let jail_id = jailer::jail_instance_id(id);
    let chroot_root = jailer::chroot_root(&jail.chroot_base_dir, &config.firecracker_bin, &jail_id);
    let instance_dir = jailer::instance_dir(&chroot_root);

    match spawn_jailed_inner(config, jail, &jail_id, &chroot_root, log_path) {
        Ok(target) => Ok(target),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&instance_dir);
            Err(e)
        }
    }
}

fn spawn_jailed_inner(
    config: &VmConfig,
    jail: &JailLaunch,
    jail_id: &str,
    chroot_root: &Path,
    log_path: &Path,
) -> io::Result<BootTarget> {
    jailer::prepare_chroot_dir(chroot_root)?;

    let kernel = jailer::link_resource_into_jail(&config.kernel_path, chroot_root, "kernel", false)?;
    let rootfs = jailer::link_resource_into_jail(&config.rootfs_path, chroot_root, "rootfs.ext4", true)?;

    let mut drive_paths = Vec::with_capacity(config.extra_drives.len());
    for drive in &config.extra_drives {
        let jail_relative_name = format!("{}.ext4", drive.drive_id);
        let linked = jailer::link_resource_into_jail(&drive.path_on_host, chroot_root, &jail_relative_name, !drive.read_only)?;
        drive_paths.push(linked.in_jail_path);
    }

    let api_socket = chroot_root.join("api.sock");
    let vsock_socket = chroot_root.join("vsock.sock");
    let _ = std::fs::remove_file(&api_socket);
    let _ = std::fs::remove_file(&vsock_socket);

    let cgroup_limits = jailer::cgroup_limits(config.mem_size_mib, config.vcpu_count);
    let jailer_args = jailer::build_jailer_args(jail, jail_id, &config.firecracker_bin, &cgroup_limits, Path::new("/api.sock"));

    let (stdout, stderr) = console_log_stdio(log_path)?;
    let child = Command::new(&jail.jailer_bin).args(&jailer_args).stdout(stdout).stderr(stderr).spawn()?;

    Ok(BootTarget {
        child,
        api_socket,
        kernel_image_path: kernel.in_jail_path,
        rootfs_path: rootfs.in_jail_path,
        drive_paths,
        vsock_uds_path: PathBuf::from("/vsock.sock"),
        vsock_socket,
        jail_instance_dir: Some(jailer::instance_dir(chroot_root)),
    })
}

/// The Firecracker API PUT sequence that turns a freshly spawned (direct
/// or jailed) process into a running VM. Identical for both boot modes —
/// only the paths in `target` differ, already resolved by
/// `spawn_direct`/`spawn_jailed` into whatever Firecracker itself needs
/// to see them as.
fn configure_and_start(config: &VmConfig, target: &mut BootTarget) -> io::Result<()> {
    wait_for_socket(&target.api_socket, Duration::from_secs(2))?;
    let mut api = ApiClient::connect(&target.api_socket)?;

    let mut boot_args = "console=ttyS0 reboot=k panic=1 pci=off".to_string();
    if let Some(net) = &config.network {
        boot_args.push_str(&format!(" ip={}::{}:255.255.255.0::eth0:off", net.guest_ip, net.gateway_ip));
    }

    put_checked(
        &mut api,
        "/boot-source",
        &json!({
            "kernel_image_path": path_str(&target.kernel_image_path),
            "boot_args": boot_args,
        }),
    )?;

    put_checked(
        &mut api,
        "/drives/rootfs",
        &json!({
            "drive_id": "rootfs",
            "path_on_host": path_str(&target.rootfs_path),
            "is_root_device": true,
            "is_read_only": false,
        }),
    )?;

    for (drive, path) in config.extra_drives.iter().zip(target.drive_paths.iter()) {
        put_checked(
            &mut api,
            &format!("/drives/{}", drive.drive_id),
            &json!({
                "drive_id": drive.drive_id,
                "path_on_host": path_str(path),
                "is_root_device": false,
                "is_read_only": drive.read_only,
            }),
        )?;
    }

    put_checked(
        &mut api,
        "/machine-config",
        &json!({
            "vcpu_count": config.vcpu_count,
            "mem_size_mib": config.mem_size_mib,
        }),
    )?;

    if let Some(net) = &config.network {
        put_checked(
            &mut api,
            "/network-interfaces/eth0",
            &json!({
                "iface_id": "eth0",
                "guest_mac": net.guest_mac,
                "host_dev_name": net.tap_device,
            }),
        )?;
    }

    put_checked(
        &mut api,
        "/vsock",
        &json!({
            "vsock_id": "vsock0",
            "guest_cid": 3,
            "uds_path": path_str(&target.vsock_uds_path),
        }),
    )?;

    put_checked(&mut api, "/actions", &json!({"action_type": "InstanceStart"}))?;
    Ok(())
}

fn put_checked(api: &mut ApiClient, path: &str, body: &serde_json::Value) -> io::Result<()> {
    let response = api.put(path, &body.to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(io::Error::other(format!(
            "firecracker API PUT {path} -> {}: {}",
            response.status, response.body
        )));
    }
    Ok(())
}

/// Where a given VM's captured serial console (Firecracker's own
/// stdout/stderr, which carries the guest kernel's `console=ttyS0` output)
/// is written. Shared with `snapshot::resume`, which boots a fresh
/// Firecracker process the same way `boot` does.
pub(crate) fn console_log_path(id: u64) -> PathBuf {
    PathBuf::from(format!("/tmp/sandkiln-fc-{id}.log"))
}

/// Opens the console log file and returns two independent handles to it
/// for the child process's stdout/stderr — interleaved into one file the
/// way a shell's `2>&1` would, since both streams are just the same
/// serial console and splitting them buys nothing.
pub(crate) fn console_log_stdio(log_path: &Path) -> io::Result<(Stdio, Stdio)> {
    let file = std::fs::File::create(log_path)?;
    let stderr_file = file.try_clone()?;
    Ok((Stdio::from(file), Stdio::from(stderr_file)))
}

/// Points a boot/resume failure at the console log so an operator isn't
/// left guessing why a guest never came up — the log is often the only
/// evidence of a kernel panic or agent crash that happened before vsock
/// was reachable.
pub(crate) fn annotate_with_console_log(err: io::Error, log_path: &Path) -> io::Error {
    io::Error::other(format!("{err} (guest console log: {})", log_path.display()))
}

fn wait_for_socket(path: &Path, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("{path:?} never appeared")));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

fn path_str(p: &Path) -> &str {
    p.to_str().expect("non-UTF8 paths are not supported")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn console_log_path_is_keyed_by_vm_id() {
        assert_eq!(console_log_path(42), PathBuf::from("/tmp/sandkiln-fc-42.log"));
    }

    #[test]
    fn annotate_with_console_log_includes_the_log_path_in_the_message() {
        let err = io::Error::other("boot-source PUT failed");
        let annotated = annotate_with_console_log(err, Path::new("/tmp/sandkiln-fc-7.log"));
        let message = annotated.to_string();
        assert!(message.contains("boot-source PUT failed"), "message was: {message}");
        assert!(message.contains("/tmp/sandkiln-fc-7.log"), "message was: {message}");
    }

    #[test]
    fn console_log_stdio_opens_a_writable_file_both_handles_share() {
        let dir = std::env::temp_dir().join(format!("sandkiln-console-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("console.log");

        let (stdout, stderr) = console_log_stdio(&log_path).unwrap();
        // Both Stdio handles reference the same file — write through each
        // independently and confirm the file exists with content, the way
        // a spawned child interleaving stdout/stderr into it would.
        drop(stdout);
        drop(stderr);
        assert!(log_path.exists());

        let mut contents = String::new();
        std::fs::File::open(&log_path).unwrap().read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn console_log_stdio_fails_cleanly_for_an_unwritable_directory() {
        let result = console_log_stdio(Path::new("/nonexistent-dir-for-sandkiln-tests/console.log"));
        assert!(result.is_err());
    }
}

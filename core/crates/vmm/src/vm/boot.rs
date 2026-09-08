//! Boot mechanics: turning a `VmConfig` into a running `Vm`, direct or
//! jailed. Split out of `vm/mod.rs` because it's a second, separately-
//! workable piece of "how a VM comes to exist" — the public API surface
//! (`VmConfig`, `Vm` and its lifecycle methods) shouldn't have to sit
//! next to the process-spawning and Firecracker-API-PUT-sequence details
//! that back `Vm::boot`, same reasoning [`super::snapshot`] already gives
//! for itself.

use super::{console_log_stdio, path_str, put_checked, wait_for_socket, RateLimiter, Vm, VmConfig};
use crate::firecracker_api::ApiClient;
use crate::jailer::{self, JailLaunch};
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Everything the API-configuration sequence needs after spawning the
/// child process (direct or jailed), gathered in one place so that
/// sequence doesn't need to branch on jail-vs-direct at all — only the
/// paths in `BootTarget` differ, not the sequence of calls.
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

/// Boots a new microVM for `Vm::boot`: spawns the child (direct or
/// jailed), runs the Firecracker API configuration sequence, and cleans
/// up the spawned process/chroot on any failure partway through — a boot
/// that fails after the child exists but before `InstanceStart` succeeds
/// must not leak an orphaned process or a world-readable chroot.
pub(super) fn boot(config: &VmConfig, id: u64, log_path: &Path) -> io::Result<Vm> {
    let started = Instant::now();

    let mut target = match &config.jail {
        None => spawn_direct(config, id, log_path)?,
        Some(jail) => spawn_jailed(config, jail, id, log_path)?,
    };

    if let Err(e) = configure_and_start(config, &mut target) {
        // A boot that fails partway through the API PUT sequence still
        // has a live child process (jailer, or firecracker directly)
        // holding the console log fds and — for a jailed boot — a real
        // chroot directory with hard-linked copies of the kernel/rootfs/
        // drives. Leaving either behind is a resource leak (an orphaned
        // process for a direct boot) or a real information-disclosure
        // surface (a leftover world-readable chroot for a jailed one),
        // not just untidy state — clean up exactly like
        // `snapshot::resume` already does for the equivalent failure.
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
    Ok(Vm {
        id,
        child: target.child,
        api_socket: target.api_socket,
        vsock_socket: target.vsock_socket,
        jail_instance_dir: target.jail_instance_dir,
    })
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

    let mut rootfs_body = json!({
        "drive_id": "rootfs",
        "path_on_host": path_str(&target.rootfs_path),
        "is_root_device": true,
        "is_read_only": false,
    });
    insert_rate_limiter(&mut rootfs_body, "rate_limiter", &config.rate_limit);
    put_checked(&mut api, "/drives/rootfs", &rootfs_body)?;

    for (drive, path) in config.extra_drives.iter().zip(target.drive_paths.iter()) {
        let mut drive_body = json!({
            "drive_id": drive.drive_id,
            "path_on_host": path_str(path),
            "is_root_device": false,
            "is_read_only": drive.read_only,
        });
        insert_rate_limiter(&mut drive_body, "rate_limiter", &config.rate_limit);
        put_checked(&mut api, &format!("/drives/{}", drive.drive_id), &drive_body)?;
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
        let mut net_body = json!({
            "iface_id": "eth0",
            "guest_mac": net.guest_mac,
            "host_dev_name": net.tap_device,
        });
        insert_rate_limiter(&mut net_body, "rx_rate_limiter", &config.rate_limit);
        insert_rate_limiter(&mut net_body, "tx_rate_limiter", &config.rate_limit);
        put_checked(&mut api, "/network-interfaces/eth0", &net_body)?;
    }

    if let Some(metadata) = &config.metadata {
        if config.network.is_none() {
            return Err(io::Error::other("VmConfig::metadata requires VmConfig::network to also be set"));
        }
        // Pre-boot only, and only valid once the interface it names is
        // itself configured -- must come after the /network-interfaces
        // PUT above, not before.
        put_checked(&mut api, "/mmds/config", &json!({ "network_interfaces": ["eth0"], "version": "V2" }))?;
        put_checked(&mut api, "/mmds", metadata)?;
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

/// Inserts `rate_limit` (if set) into `body` under `key` — `key` is
/// `"rate_limiter"` for a drive body, or `"rx_rate_limiter"`/
/// `"tx_rate_limiter"` for a network-interface body (Firecracker limits
/// each direction independently even though sandkiln applies the same
/// limiter to both). A no-op when `rate_limit` is `None`, leaving the
/// body exactly as it was before this existed.
fn insert_rate_limiter(body: &mut serde_json::Value, key: &str, rate_limit: &Option<RateLimiter>) {
    if let Some(rl) = rate_limit {
        let value = serde_json::to_value(rl).expect("RateLimiter always serializes");
        body.as_object_mut().expect("body is always a JSON object").insert(key.to_string(), value);
    }
}

//! Boot mechanics: turning a `VmConfig` into a running `Vm`, direct or
//! jailed. Split out of `vm/mod.rs` as a separately-workable piece —
//! process-spawning and the Firecracker PUT sequence shouldn't sit next
//! to the public `Vm` API, same reasoning [`super::snapshot`] gives.

use super::{connect_api_with_retry, console_log_stdio, path_str, put_checked, RateLimiter, Vm, VmConfig};
use crate::jailer::{self, JailLaunch};
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Everything the API-configuration sequence needs after spawning the
/// child (direct or jailed), so that sequence never branches on
/// jail-vs-direct — only the paths here differ.
struct BootTarget {
    child: Child,
    /// Always a real host path, even jailed (`<chroot_root>/api.sock`) —
    /// the host process never enters the chroot itself.
    api_socket: PathBuf,
    /// Host-visible vsock UDS path, for `Vm::call`'s post-boot connections.
    vsock_socket: PathBuf,
    /// `kernel_image_path` to send Firecracker: a host path direct, an
    /// in-jail path (e.g. `/kernel`) jailed — Firecracker only sees the
    /// latter once jailer has `chroot()`'d.
    kernel_image_path: PathBuf,
    rootfs_path: PathBuf,
    /// Parallel to `VmConfig::extra_drives`, by index.
    drive_paths: Vec<PathBuf>,
    /// `/vsock`'s `uds_path` value — same host/in-jail distinction as
    /// `kernel_image_path`.
    vsock_uds_path: PathBuf,
    jail_instance_dir: Option<PathBuf>,
}

/// Where a boot's wall-clock time went, for the debug-level profiling
/// breakdown — a struct rather than a tuple so phases stay named.
struct BootPhases {
    /// Waiting for the API socket to accept — see
    /// [`connect_api_with_retry`] for the poll granularity.
    socket_wait: Duration,
    /// Every pre-`InstanceStart` configuration PUT.
    api_config: Duration,
    /// `InstanceStart` alone — where vCPUs/the guest kernel actually start.
    instance_start: Duration,
    /// Per-endpoint detail as `"/path=NNNus ..."` — a string since the
    /// endpoint set varies per VM and a `tracing` field can't.
    per_put: String,
}

/// Boots a new microVM for `Vm::boot`: spawns the child, runs the
/// Firecracker API configuration sequence, and cleans up the spawned
/// process/chroot on any failure partway through — must not leak an
/// orphaned process or a world-readable chroot.
pub(super) fn boot(config: &VmConfig, id: u64, log_path: &Path) -> io::Result<Vm> {
    let started = Instant::now();

    let mut target = match &config.jail {
        None => spawn_direct(config, id, log_path)?,
        Some(jail) => spawn_jailed(config, jail, id, log_path)?,
    };
    let spawn = started.elapsed();

    let phases = match configure_and_start(config, &mut target) {
        Ok(phases) => phases,
        Err(e) => {
            // A failure partway through still has a live child process
            // and, if jailed, a chroot with hard-linked kernel/rootfs/
            // drive copies. Leaving either is a resource leak or an
            // info-disclosure surface, not just untidy state — same
            // cleanup `snapshot::resume` does for the equivalent failure.
            let _ = target.child.kill();
            let _ = target.child.wait();
            let _ = std::fs::remove_file(&target.api_socket);
            let _ = std::fs::remove_file(&target.vsock_socket);
            if let Some(dir) = &target.jail_instance_dir {
                let _ = std::fs::remove_dir_all(dir);
            }
            return Err(e);
        }
    };

    // Separate from the `vm booted` event below: `boot_ms` is the number
    // an operator watches, this breakdown only matters when profiling
    // (see ROADMAP.md's Benchmarking section).
    tracing::debug!(
        vm_id = id,
        spawn_us = spawn.as_micros(),
        socket_wait_us = phases.socket_wait.as_micros(),
        api_config_us = phases.api_config.as_micros(),
        instance_start_us = phases.instance_start.as_micros(),
        per_put = %phases.per_put,
        "vm boot phase breakdown"
    );
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

/// Spawns Firecracker via jailer: builds the chroot, links every
/// resource the VM config references into it, then execs jailer. Any
/// partial failure removes the half-built chroot — not a state worth
/// leaving on disk.
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

/// The Firecracker API PUT sequence turning a freshly spawned process
/// into a running VM — identical for both boot modes, only `target`'s
/// paths differ. Bodies are built up front by [`configuration_requests`]
/// and issued in a loop here, which is what makes each PUT individually
/// timeable and keeps the ordering constraints as one readable list.
fn configure_and_start(config: &VmConfig, target: &mut BootTarget) -> io::Result<BootPhases> {
    let before_socket = Instant::now();
    let mut api = connect_api_with_retry(&target.api_socket, Duration::from_secs(2))?;
    let socket_wait = before_socket.elapsed();

    let requests = configuration_requests(config, target)?;

    let before_api_config = Instant::now();
    let mut per_put = Vec::with_capacity(requests.len());
    for (path, body) in &requests {
        let before_put = Instant::now();
        put_checked(&mut api, path, body)?;
        per_put.push(format!("{path}={}us", before_put.elapsed().as_micros()));
    }
    let api_config = before_api_config.elapsed();

    // A different kind of cost than the PUTs above: those only record
    // intent in Firecracker's config, this is where vCPUs/the guest
    // kernel actually start.
    let before_instance_start = Instant::now();
    put_checked(&mut api, "/actions", &json!({"action_type": "InstanceStart"}))?;
    let instance_start = before_instance_start.elapsed();

    Ok(BootPhases { socket_wait, api_config, instance_start, per_put: per_put.join(" ") })
}

/// Every pre-`InstanceStart` configuration PUT, in the order Firecracker
/// requires them, as `(path, body)` pairs.
fn configuration_requests(config: &VmConfig, target: &BootTarget) -> io::Result<Vec<(String, serde_json::Value)>> {
    let mut requests: Vec<(String, serde_json::Value)> = Vec::new();

    let mut boot_args = "console=ttyS0 reboot=k panic=1 pci=off".to_string();
    if let Some(net) = &config.network {
        boot_args.push_str(&format!(" ip={}::{}:255.255.255.0::eth0:off", net.guest_ip, net.gateway_ip));
    }

    requests.push((
        "/boot-source".to_string(),
        json!({
            "kernel_image_path": path_str(&target.kernel_image_path),
            "boot_args": boot_args,
        }),
    ));

    let mut rootfs_body = json!({
        "drive_id": "rootfs",
        "path_on_host": path_str(&target.rootfs_path),
        "is_root_device": true,
        "is_read_only": false,
    });
    insert_rate_limiter(&mut rootfs_body, "rate_limiter", &config.rate_limit);
    requests.push(("/drives/rootfs".to_string(), rootfs_body));

    for (drive, path) in config.extra_drives.iter().zip(target.drive_paths.iter()) {
        let mut drive_body = json!({
            "drive_id": drive.drive_id,
            "path_on_host": path_str(path),
            "is_root_device": false,
            "is_read_only": drive.read_only,
        });
        insert_rate_limiter(&mut drive_body, "rate_limiter", &config.rate_limit);
        requests.push((format!("/drives/{}", drive.drive_id), drive_body));
    }

    requests.push((
        "/machine-config".to_string(),
        json!({
            "vcpu_count": config.vcpu_count,
            "mem_size_mib": config.mem_size_mib,
        }),
    ));

    if let Some(net) = &config.network {
        let mut net_body = json!({
            "iface_id": "eth0",
            "guest_mac": net.guest_mac,
            "host_dev_name": net.tap_device,
        });
        insert_rate_limiter(&mut net_body, "rx_rate_limiter", &config.rate_limit);
        insert_rate_limiter(&mut net_body, "tx_rate_limiter", &config.rate_limit);
        requests.push(("/network-interfaces/eth0".to_string(), net_body));
    }

    if let Some(metadata) = &config.metadata {
        if config.network.is_none() {
            return Err(io::Error::other("VmConfig::metadata requires VmConfig::network to also be set"));
        }
        // Must come after /network-interfaces -- only valid once that
        // interface is configured.
        requests.push(("/mmds/config".to_string(), json!({ "network_interfaces": ["eth0"], "version": "V2" })));
        requests.push(("/mmds".to_string(), metadata.clone()));
    }

    requests.push((
        "/vsock".to_string(),
        json!({
            "vsock_id": "vsock0",
            "guest_cid": 3,
            "uds_path": path_str(&target.vsock_uds_path),
        }),
    ));

    Ok(requests)
}

/// Inserts `rate_limit` into `body` under `key` (`"rate_limiter"` for a
/// drive, `"rx_rate_limiter"`/`"tx_rate_limiter"` for network — Firecracker
/// limits each direction independently even though sandkiln applies one
/// limiter to both). No-op when `None`.
fn insert_rate_limiter(body: &mut serde_json::Value, key: &str, rate_limit: &Option<RateLimiter>) {
    if let Some(rl) = rate_limit {
        let value = serde_json::to_value(rl).expect("RateLimiter always serializes");
        body.as_object_mut().expect("body is always a JSON object").insert(key.to_string(), value);
    }
}

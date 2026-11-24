//! Boot mechanics: turning a `VmConfig` into a running `Vm`, direct or
//! jailed. Split out of `vm/mod.rs` as a separately-workable piece —
//! process-spawning and the Firecracker PUT sequence shouldn't sit next
//! to the public `Vm` API, same reasoning [`super::snapshot`] gives.

use super::jail_state::JailState;
use super::terminate::{self, Termination};
use super::{connect_api_with_retry, console_log_stdio, path_str, put_checked, RateLimiter, Vm, VmConfig};
use crate::jailer::{self, JailLaunch};
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    jail: Option<JailState>,
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
            if let Some(jail) = target.jail.as_mut() {
                jail.request_exit();
            }
            let termination = terminate::terminate_child(&mut target.child, terminate::REAP_GRACE);
            let _ = std::fs::remove_file(&target.api_socket);
            let _ = std::fs::remove_file(&target.vsock_socket);
            if let Some(jail) = &target.jail {
                jail.teardown(id);
            }
            if let Termination::StillRunning { pid, kill_error } = termination {
                return Err(terminate::unkilled_error(pid, kill_error.as_ref(), &e.to_string()));
            }
            return Err(e);
        }
    };

    // Separate from the `vm booted` event below: `boot_ms` is the number
    // an operator watches, this breakdown only matters when profiling
    // (see `docs/benchmarks.md`).
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
        started_at: Instant::now(),
        id,
        child: target.child,
        api_socket: target.api_socket,
        vsock_socket: target.vsock_socket,
        jail: target.jail,
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
        jail: None,
    })
}

/// Spawns Firecracker via jailer: builds the chroot, links every
/// resource the VM config references into it, then execs jailer. Any
/// partial failure removes the half-built chroot — not a state worth
/// leaving on disk.
fn spawn_jailed(config: &VmConfig, jail: &JailLaunch, id: u64, log_path: &Path) -> io::Result<BootTarget> {
    let jail_id = jailer::jail_instance_id(id);
    let chroot_root = jailer::chroot_root(&jail.chroot_base_dir, &jail.supervisor_bin, &jail_id);
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

    // The supervisor (jailer's exec file) runs this as Firecracker, see
    // `crates/jail-supervisor`.
    jailer::copy_binary_into_jail(&config.firecracker_bin, chroot_root, "firecracker")?;
    let kernel = jailer::link_resource_into_jail(&config.kernel_path, chroot_root, "kernel", false, jail.gid)?;
    let rootfs = jailer::link_resource_into_jail(&config.rootfs_path, chroot_root, "rootfs.ext4", true, jail.gid)?;

    let mut drive_paths = Vec::with_capacity(config.extra_drives.len());
    for drive in &config.extra_drives {
        let jail_relative_name = format!("{}.ext4", drive.drive_id);
        let linked = jailer::link_resource_into_jail(&drive.path_on_host, chroot_root, &jail_relative_name, !drive.read_only, jail.gid)?;
        drive_paths.push(linked.in_jail_path);
    }

    let api_socket = chroot_root.join("api.sock");
    let vsock_socket = chroot_root.join("vsock.sock");
    let _ = std::fs::remove_file(&api_socket);
    let _ = std::fs::remove_file(&vsock_socket);

    let cgroup_limits = jailer::cgroup_limits(config.mem_size_mib, config.vcpu_count);
    let jailer_args = jailer::build_jailer_args(jail, jail_id, &cgroup_limits, Path::new("/api.sock"));

    let (stdout, stderr) = console_log_stdio(log_path)?;
    // The piped stdin is the supervisor's stop signal (see `JailState`).
    let mut child = Command::new(&jail.jailer_bin).args(&jailer_args).stdin(Stdio::piped()).stdout(stdout).stderr(stderr).spawn()?;
    let control = child.stdin.take();

    Ok(BootTarget {
        child,
        api_socket,
        kernel_image_path: kernel.in_jail_path,
        rootfs_path: rootfs.in_jail_path,
        drive_paths,
        vsock_uds_path: PathBuf::from("/vsock.sock"),
        vsock_socket,
        jail: Some(JailState {
            instance_dir: jailer::instance_dir(chroot_root),
            cgroup_dir: jailer::cgroup_dir(&jail.supervisor_bin, jail_id),
            control,
        }),
    })
}

/// How long a jailed boot waits for the API socket to become connectable.
/// Generous next to the direct path's 2s: jailer first copies the
/// supervisor into the chroot, creates cgroups and device nodes.
/// Jailer-to-socket is well under a second when it works, so hitting this
/// means something is wrong, not slow.
const JAILED_API_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on any single read or write to Firecracker's API during boot.
/// `InstanceStart` is the slowest call (tens of milliseconds); this only
/// exists so a wedged process cannot hang a create.
const API_IO_TIMEOUT: Duration = Duration::from_secs(15);

/// Waits for a jailed Firecracker's API socket, with a deadline and with
/// early exit when the jailer/Firecracker process has already died. On
/// failure the error names the likely cause (see
/// [`jailed_api_unreachable_message`]).
fn wait_for_jailed_api(api_socket: &Path, child: &mut Child, timeout: Duration) -> io::Result<crate::firecracker_api::ApiClient> {
    use crate::firecracker_api::ApiClient;
    let deadline = Instant::now() + timeout;
    let mut interval = Duration::from_micros(500);
    loop {
        match ApiClient::connect(api_socket) {
            Ok(api) => return Ok(api),
            Err(last) => {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(io::Error::other(format!(
                        "the jailer/Firecracker process exited ({status}) before its API socket {} became reachable; \
                         see the guest console log for the jailer's own error",
                        api_socket.display()
                    )));
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, jailed_api_unreachable_message(api_socket, timeout, &last)));
                }
                std::thread::sleep(interval);
                interval = (interval * 2).min(Duration::from_millis(20));
            }
        }
    }
}

/// The operator-facing explanation when a jailed Firecracker's API socket
/// never became reachable. A permission error means the daemon is not
/// allowed into the jail: jailer makes the chroot root mode 0700 and owned
/// by the per-VM uid, and only the jail supervisor's widening to the jail
/// group (which the daemon's user must belong to) makes it reachable. That
/// was finding 32 in `docs/findings.md`.
fn jailed_api_unreachable_message(api_socket: &Path, timeout: Duration, last_error: &io::Error) -> String {
    let cause = match last_error.kind() {
        io::ErrorKind::PermissionDenied => {
            "permission denied: the daemon is not allowed into the jail. The jailer makes the chroot root mode 0700 and the jail \
             supervisor opens it to the jail group, so the daemon's user must be a member of that group in the session that started \
             it (check `id`; see scripts/host-setup/prepare-jailer.sh and finding 32 in docs/findings.md)"
        }
        io::ErrorKind::NotFound => {
            "the socket never appeared: the jailer or the supervisor likely failed to start (a jailer binary that is not setuid root, \
             cgroup v2 missing, a bad uid range, or a supervisor that is not a static binary); see the console log"
        }
        _ => "the socket exists but refused the connection",
    };
    format!("jailed Firecracker API socket {} was not reachable within {timeout:?}: {cause} (last error: {last_error})", api_socket.display())
}

/// The Firecracker API PUT sequence turning a freshly spawned process
/// into a running VM — identical for both boot modes, only `target`'s
/// paths differ. Bodies are built up front by [`configuration_requests`]
/// and issued in a loop here, which is what makes each PUT individually
/// timeable and keeps the ordering constraints as one readable list.
fn configure_and_start(config: &VmConfig, target: &mut BootTarget) -> io::Result<BootPhases> {
    let before_socket = Instant::now();
    let mut api = if target.jail.is_some() {
        wait_for_jailed_api(&target.api_socket, &mut target.child, JAILED_API_TIMEOUT)?
    } else {
        connect_api_with_retry(&target.api_socket, Duration::from_secs(2))?
    };
    // A Firecracker that accepts the connection but never answers must not
    // hang the create: every configuration call is bounded.
    api.set_io_timeout(Some(API_IO_TIMEOUT))?;
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

/// Arguments every sandbox kernel boots with, after the fixed
/// `console=ttyS0 reboot=k panic=1 pci=off` base.
///
/// * `quiet`: the serial console is an emulated 16550 UART, so each byte the
///   kernel logs is a VM exit. Dropping boot logging below warnings saved
///   about 90ms to first command. Warnings, errors and panics still print.
/// * `loop.max_loop=1`: the loop driver otherwise pre-creates its default set
///   of devices at init, measured at 124ms (`initcall_debug`), about 60% of the
///   guest kernel's boot. Loop devices are still created on demand, verified by
///   mounting three loop images in a guest.
///
/// Measured end to end (create to first successful exec, 15 cold sandboxes each,
/// 2026-11-24): 441ms without these, 237ms with them. See the optimization
/// post on guest boot time.
const DEFAULT_KERNEL_ARGS: &str = "quiet loop.max_loop=1";

/// The full guest kernel command line: fixed base, `DEFAULT_KERNEL_ARGS`,
/// `ip=` when networked, then the operator's `extra_boot_args` last so a later
/// argument can override an earlier one (e.g. `loglevel=7` to see boot logs).
fn kernel_command_line(config: &VmConfig) -> String {
    let mut args = format!("console=ttyS0 reboot=k panic=1 pci=off {DEFAULT_KERNEL_ARGS}");
    if let Some(net) = &config.network {
        args.push_str(&format!(" ip={}::{}:255.255.255.0::eth0:off", net.guest_ip, net.gateway_ip));
    }
    let extra = config.extra_boot_args.trim();
    if !extra.is_empty() {
        args.push(' ');
        args.push_str(extra);
    }
    args
}

/// Every pre-`InstanceStart` configuration PUT, in the order Firecracker
/// requires them, as `(path, body)` pairs.
fn configuration_requests(config: &VmConfig, target: &BootTarget) -> io::Result<Vec<(String, serde_json::Value)>> {
    let mut requests: Vec<(String, serde_json::Value)> = Vec::new();

    let boot_args = kernel_command_line(config);

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

#[cfg(test)]
mod jailed_wait_tests {
    use super::*;

    #[test]
    fn permission_denied_is_explained_as_the_unreachable_chroot() {
        let msg = jailed_api_unreachable_message(Path::new("/j/root/api.sock"), Duration::from_secs(10), &io::Error::from(io::ErrorKind::PermissionDenied));
        assert!(msg.contains("finding 32") && msg.contains("jail group") && msg.contains("/j/root/api.sock"), "{msg}");
    }

    #[test]
    fn a_missing_socket_points_at_jailer_startup_failure() {
        let msg = jailed_api_unreachable_message(Path::new("/s"), Duration::from_secs(1), &io::Error::from(io::ErrorKind::NotFound));
        assert!(msg.contains("never appeared") && msg.contains("setuid") && msg.contains("static"), "{msg}");
    }

    #[test]
    fn wait_fails_fast_when_the_process_already_exited() {
        let mut child = Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        let started = Instant::now();
        let err = match wait_for_jailed_api(Path::new("/nonexistent/api.sock"), &mut child, Duration::from_secs(30)) {
            Ok(_) => panic!("connected to nothing"),
            Err(e) => e,
        };
        assert!(started.elapsed() < Duration::from_secs(5), "must not wait out the deadline for a dead process");
        assert!(err.to_string().contains("exited"), "{err}");
    }

    #[test]
    fn wait_times_out_with_a_clear_error_when_the_socket_never_appears() {
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let started = Instant::now();
        let err = match wait_for_jailed_api(Path::new("/nonexistent/api.sock"), &mut child, Duration::from_millis(200)) {
            Ok(_) => panic!("connected to nothing"),
            Err(e) => e,
        };
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(err.to_string().contains("not reachable within"), "{err}");
    }
}

#[cfg(test)]
mod kernel_args_tests {
    use super::*;
    use crate::vm::NetworkConfig;
    use std::path::PathBuf;

    fn config(network: Option<NetworkConfig>, extra: &str) -> VmConfig {
        VmConfig {
            firecracker_bin: PathBuf::new(),
            kernel_path: PathBuf::new(),
            rootfs_path: PathBuf::new(),
            vcpu_count: 1,
            mem_size_mib: 128,
            extra_boot_args: extra.to_string(),
            network,
            extra_drives: vec![],
            jail: None,
            rate_limit: None,
            metadata: None,
        }
    }

    #[test]
    fn defaults_include_the_boot_time_tuning_and_the_fixed_base() {
        let line = kernel_command_line(&config(None, ""));
        assert!(line.starts_with("console=ttyS0 reboot=k panic=1 pci=off"));
        assert!(line.contains("quiet") && line.contains("loop.max_loop=1"));
        assert!(!line.contains("ip="), "no ip= without a network");
    }

    #[test]
    fn network_adds_a_static_ip_and_extra_args_come_last() {
        let net = NetworkConfig {
            tap_device: "sktap0".into(),
            guest_ip: "172.16.0.8".parse().unwrap(),
            gateway_ip: "172.16.0.1".parse().unwrap(),
            guest_mac: "AA:FC:00:00:08:08".into(),
        };
        let line = kernel_command_line(&config(Some(net), "  loglevel=7 "));
        assert!(line.contains(" ip=172.16.0.8::172.16.0.1:255.255.255.0::eth0:off"));
        assert!(line.ends_with(" loglevel=7"), "operator args must win by coming last: {line}");
    }
}

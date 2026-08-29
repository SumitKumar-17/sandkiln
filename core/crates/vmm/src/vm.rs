//! A single Firecracker microVM's lifecycle: boot, talk to its guest
//! agent, tear down. This is the Rust equivalent of what
//! `scripts/boot-test-vm.sh` does by hand — the daemon drives this
//! directly instead of shelling out.

use crate::firecracker_api::ApiClient;
use crate::vsock_client;
use sandkiln_protocol::{Request, Response, AGENT_PORT};
use serde_json::json;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
}

pub struct Vm {
    id: u64,
    child: Child,
    api_socket: PathBuf,
    vsock_socket: PathBuf,
}

impl Vm {
    pub fn boot(config: &VmConfig) -> io::Result<Self> {
        let started = Instant::now();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let api_socket = PathBuf::from(format!("/tmp/sandkiln-fc-{id}.sock"));
        let vsock_socket = PathBuf::from(format!("/tmp/sandkiln-vsock-{id}.sock"));
        let _ = std::fs::remove_file(&api_socket);
        let _ = std::fs::remove_file(&vsock_socket);

        let child = Command::new(&config.firecracker_bin)
            .arg("--api-sock")
            .arg(&api_socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        wait_for_socket(&api_socket, Duration::from_secs(2))?;
        let mut api = ApiClient::connect(&api_socket)?;

        let mut boot_args = "console=ttyS0 reboot=k panic=1 pci=off".to_string();
        if let Some(net) = &config.network {
            boot_args.push_str(&format!(
                " ip={}::{}:255.255.255.0::eth0:off",
                net.guest_ip, net.gateway_ip
            ));
        }

        put_checked(
            &mut api,
            "/boot-source",
            &json!({
                "kernel_image_path": path_str(&config.kernel_path),
                "boot_args": boot_args,
            }),
        )?;

        put_checked(
            &mut api,
            "/drives/rootfs",
            &json!({
                "drive_id": "rootfs",
                "path_on_host": path_str(&config.rootfs_path),
                "is_root_device": true,
                "is_read_only": false,
            }),
        )?;

        for drive in &config.extra_drives {
            put_checked(
                &mut api,
                &format!("/drives/{}", drive.drive_id),
                &json!({
                    "drive_id": drive.drive_id,
                    "path_on_host": path_str(&drive.path_on_host),
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
                "uds_path": path_str(&vsock_socket),
            }),
        )?;

        put_checked(&mut api, "/actions", &json!({"action_type": "InstanceStart"}))?;

        tracing::info!(vm_id = id, pid = child.id(), boot_ms = started.elapsed().as_millis(), "vm booted");
        Ok(Self { id, child, api_socket, vsock_socket })
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
        tracing::info!(vm_id = self.id, "vm stopped");
        Ok(())
    }
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

//! `Vm::pause`/`snapshot`/`resume`: save a running microVM's full state
//! (device state + guest memory) to disk and later boot a fresh
//! Firecracker process straight from that save point instead of a kernel
//! boot. See `daemon::routes_snapshot` for the HTTP surface built on this.

use super::{path_str, put_checked, Vm, NEXT_ID};
use crate::firecracker_api::ApiClient;
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

/// Configuration for booting a new microVM by loading a previously taken
/// snapshot instead of doing a fresh kernel boot. See `Vm::resume`.
pub struct ResumeConfig {
    pub firecracker_bin: PathBuf,
    /// The state file written by `Vm::snapshot`'s `snapshot_path`.
    pub snapshot_path: PathBuf,
    /// The guest-memory file written by `Vm::snapshot`'s `mem_path`.
    pub mem_file_path: PathBuf,
}

impl Vm {
    /// Pauses a running microVM. Firecracker requires this before
    /// `/snapshot/create` will accept a request — an un-paused VM's
    /// memory and device state are a moving target.
    pub fn pause(&self) -> io::Result<()> {
        let mut api = ApiClient::connect(&self.api_socket)?;
        patch_checked(&mut api, "/vm", &json!({"state": "Paused"}))?;
        tracing::info!(vm_id = self.id, "vm paused");
        Ok(())
    }

    /// Snapshots a paused microVM's full state (device state + guest
    /// memory) to disk. Call `pause()` first. Records the rootfs drive's
    /// *host path*, not its contents — the backing file must still exist
    /// at that path whenever this snapshot is resumed.
    pub fn snapshot(&self, mem_path: &Path, snapshot_path: &Path) -> io::Result<()> {
        let mut api = ApiClient::connect(&self.api_socket)?;
        put_checked(
            &mut api,
            "/snapshot/create",
            &json!({
                "mem_file_path": path_str(mem_path),
                "snapshot_path": path_str(snapshot_path),
            }),
        )?;
        tracing::info!(vm_id = self.id, "vm snapshotted");
        Ok(())
    }

    /// Boots a VM from a previously taken snapshot instead of a fresh
    /// kernel boot — skips `/boot-source`/`/drives`/`/machine-config`/
    /// `/network-interfaces` entirely, reconstructed from the snapshot's
    /// device state. `resume_vm: true` starts it as part of the same
    /// call, no separate `InstanceStart`.
    ///
    /// The rootfs file and tap device are referenced by host path/name,
    /// not by value, inside the snapshot — moving either silently breaks
    /// a resume rather than erroring (see the Persistence model doc).
    /// `ResumeConfig` deliberately has no `network` field: the daemon
    /// holds the original `Lease` across a snapshot and hands that exact
    /// one back. The vsock socket path is the one exception — host-side
    /// plumbing the guest never sees, safely reassigned fresh via
    /// Firecracker's `vsock_override`, same as `boot()`.
    pub fn resume(config: &ResumeConfig) -> io::Result<Self> {
        let started = Instant::now();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let api_socket = PathBuf::from(format!("/tmp/sandkiln-fc-{id}.sock"));
        let vsock_socket = PathBuf::from(format!("/tmp/sandkiln-vsock-{id}.sock"));
        let log_path = super::console_log_path(id);
        let _ = std::fs::remove_file(&api_socket);
        let _ = std::fs::remove_file(&vsock_socket);

        let (stdout, stderr) = super::console_log_stdio(&log_path)?;
        let mut child = Command::new(&config.firecracker_bin)
            .arg("--api-sock")
            .arg(&api_socket)
            .stdout(stdout)
            .stderr(stderr)
            .spawn()?;

        let result = super::connect_api_with_retry(&api_socket, Duration::from_secs(2)).and_then(|mut api| {
            put_checked(
                &mut api,
                "/snapshot/load",
                &json!({
                    "snapshot_path": path_str(&config.snapshot_path),
                    "mem_backend": {
                        "backend_type": "File",
                        "backend_path": path_str(&config.mem_file_path),
                    },
                    "resume_vm": true,
                    "vsock_override": {
                        "uds_path": path_str(&vsock_socket),
                    },
                }),
            )
        });

        if let Err(e) = result {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&api_socket);
            let _ = std::fs::remove_file(&vsock_socket);
            return Err(super::annotate_with_console_log(e, &log_path));
        }

        tracing::info!(
            vm_id = id,
            pid = child.id(),
            resume_ms = started.elapsed().as_millis(),
            "vm resumed from snapshot"
        );
        // Always spawns directly, never jailed — jailer only covers
        // `Vm::boot`. A jailed sandbox's snapshot bakes in in-jail paths
        // that would need relinking into a fresh chroot; `routes_snapshot`
        // refuses to attempt this yet rather than half-support it.
        Ok(Self { id, child, api_socket, vsock_socket, jail_instance_dir: None })
    }
}

fn patch_checked(api: &mut ApiClient, path: &str, body: &serde_json::Value) -> io::Result<()> {
    let response = api.patch(path, &body.to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(io::Error::other(format!(
            "firecracker API PATCH {path} -> {}: {}",
            response.status, response.body
        )));
    }
    Ok(())
}

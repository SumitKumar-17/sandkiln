//! `Vm::pause`/`snapshot`/`resume`: save a running microVM's full state
//! (device state + guest memory) to disk and later boot a fresh
//! Firecracker process straight from that save point instead of a kernel
//! boot. See `daemon::routes_snapshot` for the HTTP surface built on this.

use super::{path_str, put_checked, Vm, NEXT_ID};
use crate::firecracker_api::ApiClient;
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    /// memory) to disk. Call `pause()` first.
    ///
    /// The snapshot records the rootfs drive's *host path*, not its
    /// contents — the backing file isn't copied into either output file,
    /// so it must still exist at the same path whenever this snapshot is
    /// resumed via `Vm::resume`.
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

    /// Boots a new microVM by loading a previously taken snapshot instead
    /// of a fresh kernel boot — skips `/boot-source`, `/drives`,
    /// `/machine-config`, and `/network-interfaces` entirely, since all of
    /// that is reconstructed from the snapshot's own device state.
    /// `resume_vm: true` in the load request starts the VM running as
    /// part of the same call, so there's no separate `InstanceStart`
    /// action the way `boot()` needs.
    ///
    /// Two things the snapshot references by host path rather than by
    /// value, which the caller is responsible for getting right:
    /// - The rootfs drive's backing file (see `snapshot()`'s doc comment)
    ///   must still be at the path it had when snapshotted.
    /// - The network tap device: the guest's IP/MAC were finalized via
    ///   kernel boot args at the *original* boot and are frozen in the
    ///   snapshotted memory image, so whatever host tap device is
    ///   attached under the same name the VM had at snapshot time is what
    ///   the resumed guest will keep using — there's no way to hand it a
    ///   fresh lease here (Firecracker's `network_overrides` can rename
    ///   the *host* side of an interface on load, but the guest-visible
    ///   IP/MAC are already-booted guest OS state we can't reach, so a
    ///   caller reusing a fresh lease's tap device would just leave the
    ///   guest talking to a device nothing is listening on). This module
    ///   deliberately doesn't take a `network` field on `ResumeConfig` —
    ///   the daemon holds the original `Lease` on the sandbox's behalf
    ///   across a snapshot instead of releasing it, and simply hands the
    ///   same one to the resumed sandbox.
    ///
    /// The vsock socket path is the one exception: it's a host-side
    /// listening path the guest agent never sees, so it's safe to
    /// reassign fresh via Firecracker's `vsock_override` — which is what
    /// this does, generating a new path the same way `boot()` does.
    pub fn resume(config: &ResumeConfig) -> io::Result<Self> {
        let started = Instant::now();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let api_socket = PathBuf::from(format!("/tmp/sandkiln-fc-{id}.sock"));
        let vsock_socket = PathBuf::from(format!("/tmp/sandkiln-vsock-{id}.sock"));
        let _ = std::fs::remove_file(&api_socket);
        let _ = std::fs::remove_file(&vsock_socket);

        let mut child = Command::new(&config.firecracker_bin)
            .arg("--api-sock")
            .arg(&api_socket)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let result = super::wait_for_socket(&api_socket, Duration::from_secs(2)).and_then(|()| {
            let mut api = ApiClient::connect(&api_socket)?;
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
            return Err(e);
        }

        tracing::info!(
            vm_id = id,
            pid = child.id(),
            resume_ms = started.elapsed().as_millis(),
            "vm resumed from snapshot"
        );
        Ok(Self { id, child, api_socket, vsock_socket })
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

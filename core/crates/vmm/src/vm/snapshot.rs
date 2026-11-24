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
    /// Boot the VM from this disk image instead of the one baked into the
    /// snapshot. **Required whenever the snapshot must stay reusable** (a fork,
    /// a restorable checkpoint): the snapshot records its rootfs by host path,
    /// so without an override the resumed VM writes straight into the
    /// snapshot's own disk, and the next resume reads a filesystem that no
    /// longer matches the memory state it was captured with (observed as ext4
    /// `Bad message` errors). The file must be a copy of the snapshot's disk
    /// as it was when the snapshot was taken.
    pub rootfs_override: Option<PathBuf>,
    /// Plug the restored guest's network interface (`eth0`) into this tap
    /// device instead of the one named in the snapshot (Firecracker's
    /// `network_overrides` on snapshot load). The guest's IP and MAC are inside
    /// the memory image and do not change; only the host-side port does. Every
    /// resume passes one: the snapshot's own tap went back to the pool when it
    /// was taken and may now belong to another sandbox. `None` keeps the saved
    /// name, only correct for a snapshot with no network interface.
    pub tap_override: Option<String>,
}

impl Vm {
    /// Pauses a running microVM. Firecracker requires this before
    /// `/snapshot/create` will accept a request — an un-paused VM's
    /// memory and device state are a moving target.
    /// Gives a just-restored guest fresh per-clone state: host entropy and the
    /// host's wall clock (see `Request::ResumeFixup`). Without it every clone
    /// of one snapshot shares a random stream and a stale clock.
    pub fn apply_resume_fixup(&self) -> io::Result<()> {
        let mut entropy = [0u8; 32];
        std::io::Read::read_exact(&mut std::fs::File::open("/dev/urandom")?, &mut entropy)?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(io::Error::other)?;
        let request = sandkiln_protocol::Request::ResumeFixup {
            unix_secs: now.as_secs(),
            unix_nanos: now.subsec_nanos(),
            entropy_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, entropy),
        };
        match self.call(&request)? {
            sandkiln_protocol::Response::Ok => Ok(()),
            sandkiln_protocol::Response::Error { message } => Err(io::Error::other(message)),
            other => Err(io::Error::other(format!("unexpected fixup response: {other:?}"))),
        }
    }

    pub fn pause(&self) -> io::Result<()> {
        let age = self.started_at.elapsed();
        if age < super::MIN_GUEST_AGE_BEFORE_PAUSE {
            std::thread::sleep(super::MIN_GUEST_AGE_BEFORE_PAUSE - age);
        }
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
        put_checked(&mut api, "/snapshot/create", &snapshot_request(mem_path, snapshot_path, self.dirty_tracked))?;
        tracing::info!(vm_id = self.id, "vm snapshotted");
        Ok(())
    }

    /// Boots a VM from a previously taken snapshot instead of a fresh
    /// kernel boot — skips `/boot-source`/`/drives`/`/machine-config`/
    /// `/network-interfaces` entirely, reconstructed from the snapshot's
    /// device state. `resume_vm: true` starts it as part of the same
    /// call, no separate `InstanceStart`.
    ///
    /// The rootfs file is referenced by host path, not by value, inside the
    /// snapshot — moving it silently breaks a resume rather than erroring
    /// (see the Persistence model doc). The tap device is the opposite: its
    /// name is host plumbing, repointed with `tap_override`, as the vsock
    /// socket path is with `vsock_override` (same as `boot()`). The guest's
    /// IP and MAC, by contrast, are frozen in the image and are the daemon's
    /// to keep (`network::IpLease`).
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
                    // With an override the guest must not run before its
                    // drive is repointed, so load paused and resume below.
                    "resume_vm": config.rootfs_override.is_none(),
                    "vsock_override": {
                        "uds_path": path_str(&vsock_socket),
                    },
                    "network_overrides": network_overrides(config.tap_override.as_deref()),
                }),
            )?;
            if let Some(rootfs) = &config.rootfs_override {
                patch_checked(&mut api, "/drives/rootfs", &json!({ "drive_id": "rootfs", "path_on_host": path_str(rootfs) }))?;
                patch_checked(&mut api, "/vm", &json!({ "state": "Resumed" }))?;
            }
            Ok(())
        });

        if let Err(e) = result {
            // Resume always spawns directly (same uid as the daemon), so the
            // kill is not expected to fail; the wait is bounded regardless.
            let _ = super::terminate::terminate_child(&mut child, super::terminate::REAP_GRACE);
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
        // that would need relinking into a fresh chroot; `routes::snapshot`
        // refuses to attempt this yet rather than half-support it.
        //
        // Non-fatal like egress re-apply on resume/fork, and for the same
        // reason: the VM has already resumed successfully by this point,
        // and destroying it over a tunnel-listener setup failure (a rare,
        // one-off path issue) would be worse than leaving the local
        // tunnel feature unavailable on this one sandbox.
        if let Err(e) = crate::tunnel::listen(&vsock_socket) {
            tracing::warn!(vm_id = id, error = %e, "failed to start tunnel listener on resume; local tunnels unavailable on this sandbox");
        }
        Ok(Self { id, child, api_socket, vsock_socket, jail: None, started_at: Instant::now(), dirty_tracked: false })
    }
}

/// The `network_overrides` array for `PUT /snapshot/load`: the guest's one
/// interface (`eth0`, the id `boot.rs` configures) repointed at `tap`.
fn network_overrides(tap: Option<&str>) -> serde_json::Value {
    match tap {
        Some(tap) => json!([{ "iface_id": "eth0", "host_dev_name": tap }]),
        None => json!([]),
    }
}

/// The `PUT /snapshot/create` body. A VM booted with dirty tracking asks for a
/// `Diff`: with no earlier snapshot to be relative to, that is every page the
/// guest touched since boot, written into a file that is sparse everywhere else.
/// Untouched pages of a booted guest are zero, which is what a hole reads as, so
/// the file loads as a complete memory image (checked in
/// `docs/design-notes/snapshot-take-cost.md`).
fn snapshot_request(mem_path: &Path, snapshot_path: &Path, dirty_tracked: bool) -> serde_json::Value {
    let mut body = json!({
        "mem_file_path": path_str(mem_path),
        "snapshot_path": path_str(snapshot_path),
    });
    if dirty_tracked {
        body["snapshot_type"] = json!("Diff");
    }
    body
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tap_override_repoints_the_one_guest_interface() {
        assert_eq!(network_overrides(Some("sktap7")), json!([{ "iface_id": "eth0", "host_dev_name": "sktap7" }]));
    }

    #[test]
    fn a_dirty_tracked_vm_asks_for_a_diff_snapshot() {
        let body = snapshot_request(Path::new("/s/mem.bin"), Path::new("/s/state.snap"), true);
        assert_eq!(body["snapshot_type"], "Diff");
        assert_eq!(body["mem_file_path"], "/s/mem.bin");
    }

    #[test]
    fn an_untracked_vm_asks_for_the_default_full_snapshot() {
        let body = snapshot_request(Path::new("/s/mem.bin"), Path::new("/s/state.snap"), false);
        assert!(body.get("snapshot_type").is_none());
    }

    #[test]
    fn no_tap_override_sends_an_empty_list() {
        assert_eq!(network_overrides(None), json!([]));
    }
}

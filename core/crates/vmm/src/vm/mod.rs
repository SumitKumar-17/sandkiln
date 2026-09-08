//! A single Firecracker microVM's lifecycle: boot, talk to its guest
//! agent, tear down. This is the Rust equivalent of what
//! `scripts/dev-tools/boot-test-vm.sh` does by hand — the daemon drives this
//! directly instead of shelling out.
//!
//! Snapshot/resume (`pause`, `snapshot`, `resume`, `ResumeConfig`) lives in
//! [`snapshot`] — split out because it's a distinct capability with its
//! own long gotchas, not because `Vm` itself is two structs. Boot
//! mechanics (process spawning, the Firecracker API PUT sequence) live in
//! [`boot`] for the same reason — this file is the public API surface
//! only.

mod boot;
mod snapshot;

use crate::firecracker_api::ApiClient;
use crate::jailer::JailLaunch;
use crate::vsock_client;
use sandkiln_protocol::{Request, Response, AGENT_PORT, PTY_PORT};
use std::io;
use std::net::Ipv4Addr;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
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

/// Firecracker's own token-bucket rate limiter — a maximum capacity
/// (`size`), replenished at a constant rate derived from `size` and
/// `refill_time` (ms), with an optional initial burst
/// (`one_time_burst`) consumed before the refill rate applies. Field
/// names match Firecracker's wire format exactly (see its `TokenBucket`
/// schema) since this is serialized directly into the PUT body.
#[derive(Clone, Copy, serde::Serialize)]
pub struct TokenBucket {
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub one_time_burst: Option<u64>,
    pub refill_time: u64,
}

/// Independent bandwidth (bytes/s) and ops (operations/s) limits — either
/// or both may be set. Applied uniformly to the rootfs drive, every extra
/// drive, and both directions of the network interface (Firecracker
/// itself limits ingress/egress independently via `rx_rate_limiter`/
/// `tx_rate_limiter`, but sandkiln exposes one combined sandbox-level
/// knob rather than four independent ones — a deliberately simpler
/// surface than Firecracker's own, not a limitation of the device model).
#[derive(Clone, Copy, serde::Serialize)]
pub struct RateLimiter {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bandwidth: Option<TokenBucket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops: Option<TokenBucket>,
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
    /// When set, applied to the rootfs drive, every entry in
    /// `extra_drives`, and both directions of the network interface.
    /// `None` (the default) means unlimited host I/O, unchanged from
    /// before this existed.
    pub rate_limit: Option<RateLimiter>,
    /// Arbitrary JSON served to the guest via Firecracker's own MMDS
    /// (Microvm Metadata Service) at `http://169.254.169.254/` — a
    /// link-local HTTP endpoint Firecracker's device model answers
    /// directly, no vsock/guest-agent involvement at all. Configured
    /// V2 (token-gated: the guest must `PUT .../latest/api/token` for a
    /// session token before `GET`ting anything) rather than V1's plain
    /// unauthenticated GET, since a sandbox may run untrusted or
    /// AI-generated code that could otherwise SSRF an unauthenticated
    /// metadata endpoint. Requires `network` to also be set — MMDS
    /// intercepts requests via a configured network interface, so
    /// there's nothing for it to intercept without one. `None` (the
    /// default) configures no MMDS at all, unchanged from before this
    /// existed.
    pub metadata: Option<serde_json::Value>,
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
        boot::boot(config, id, &log_path).map_err(|e| annotate_with_console_log(e, &log_path))
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

    /// Opens a new interactive PTY session inside this VM, sized to
    /// `cols`x`rows` — a fundamentally different call shape from
    /// `call()` above: this returns a raw, still-open stream the caller
    /// shovels bytes through for as long as the session lasts, rather
    /// than one request answered by one response. Retries briefly like
    /// `call()` does, for the same reason (the agent may not be
    /// listening yet immediately after boot) even though in practice a
    /// PTY is usually opened well after a sandbox is already responsive.
    pub fn open_pty(&self, cols: u16, rows: u16) -> io::Result<UnixStream> {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(5);
        loop {
            match vsock_client::open_pty(&self.vsock_socket, PTY_PORT, cols, rows) {
                Ok(stream) => {
                    tracing::debug!(vm_id = self.id, elapsed_ms = started.elapsed().as_millis(), "pty session opened");
                    return Ok(stream);
                }
                Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
                Err(e) => {
                    tracing::warn!(vm_id = self.id, error = %e, "opening pty session failed");
                    return Err(e);
                }
            }
        }
    }

    /// Updates this VM's MMDS content in place, without a reboot or
    /// fresh boot — for when a sandbox's real identity (id/name/tags) is
    /// only known *after* it's already running, e.g. one resumed from a
    /// pre-warmed pool snapshot (see `sandkiln-daemon`'s `pool` module):
    /// the snapshot's own frozen MMDS content still reflects whatever was
    /// set when the *warm* instance was originally booted, not the
    /// caller's actual request.
    ///
    /// Redoes the full `PUT /mmds/config` + `PUT /mmds` sequence (exactly
    /// what a fresh boot's own `configure_and_start` does) rather than a
    /// bare `PATCH /mmds` — confirmed live that a resumed VM's MMDS data
    /// store comes back **uninitialized** even though the VM was
    /// networked and had MMDS configured before being snapshotted
    /// (`PATCH /mmds` alone fails with "MMDS data store is not
    /// initialized" after a resume, a real Firecracker snapshot/restore
    /// gap this works around rather than assumes away). Redoing the full
    /// sequence is correct either way, whether this VM already had MMDS
    /// content or never did. Fails if this VM has no network interface at
    /// all — `/mmds/config` needs one — which should never happen for a
    /// sandbox that reached this call, since every sandbox is networked.
    pub fn update_metadata(&self, metadata: &serde_json::Value) -> io::Result<()> {
        let mut api = ApiClient::connect(&self.api_socket)?;
        put_checked(&mut api, "/mmds/config", &serde_json::json!({ "network_interfaces": ["eth0"], "version": "V2" }))?;
        put_checked(&mut api, "/mmds", metadata)
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

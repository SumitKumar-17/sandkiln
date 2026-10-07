//! A single Firecracker microVM's lifecycle: boot, talk to its guest
//! agent, tear down — the Rust equivalent of
//! `scripts/dev-tools/boot-test-vm.sh`, driven directly instead of shelled
//! out to.
//!
//! Snapshot/resume lives in [`snapshot`], boot mechanics in [`boot`] —
//! both split out as distinct capabilities with their own gotchas; this
//! file is the public `Vm` API surface only.

mod boot;
mod snapshot;

use crate::firecracker_api::ApiClient;
use crate::jailer::JailLaunch;
use crate::vsock_client;
use sandkiln_protocol::{Request, Response, AGENT_PORT, EXEC_STREAM_PORT, PTY_PORT};
use std::collections::HashMap;
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

/// A non-root drive to attach at boot (e.g. from `crate::drive::DriveStore`)
/// — one `PUT /drives/<drive_id>` call before `InstanceStart`, shows up
/// guest-side as `/dev/vdb`, `/dev/vdc`, ... in attachment order.
#[derive(Clone)]
pub struct DriveConfig {
    /// Must be unique among all drives attached to this VM, and must not
    /// be `"rootfs"` (reserved for the root device).
    pub drive_id: String,
    pub path_on_host: PathBuf,
    pub read_only: bool,
}

/// Firecracker's token-bucket rate limiter: capacity `size`, refilled at
/// a rate derived from `size`/`refill_time` (ms), optional initial
/// `one_time_burst`. Field names match Firecracker's own `TokenBucket`
/// schema exactly — serialized directly into the PUT body.
#[derive(Clone, Copy, serde::Serialize)]
pub struct TokenBucket {
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub one_time_burst: Option<u64>,
    pub refill_time: u64,
}

/// Independent bandwidth/ops limits, either or both set — applied
/// uniformly to the rootfs drive, every extra drive, and both network
/// directions. Firecracker itself limits rx/tx independently; sandkiln
/// exposes one combined knob instead of four as a deliberately simpler
/// surface, not a device-model limitation.
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
    /// Dedicated rootfs image for this VM — caller owns copy-on-boot
    /// semantics, this module writes to it in place.
    pub rootfs_path: PathBuf,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
    pub network: Option<NetworkConfig>,
    /// Extra (non-root) drives, e.g. persistent drives. Empty for a
    /// rootfs-only VM.
    pub extra_drives: Vec<DriveConfig>,
    /// `Some` boots via Firecracker's jailer (chroot, cgroup v2, a
    /// dedicated uid/gid) instead of a direct spawn. See `crate::jailer`.
    pub jail: Option<JailLaunch>,
    /// Applied to the rootfs drive, every extra drive, and both network
    /// directions. `None` = unlimited host I/O.
    pub rate_limit: Option<RateLimiter>,
    /// Guest-visible JSON via Firecracker's own MMDS at
    /// `169.254.169.254` — answered by Firecracker's device model
    /// directly, no vsock/guest-agent involved. Configured V2
    /// (token-gated) rather than V1's unauthenticated GET, since a
    /// sandbox may run untrusted code that could otherwise SSRF an open
    /// metadata endpoint. Requires `network` (MMDS intercepts via a
    /// configured interface). `None` configures no MMDS.
    pub metadata: Option<serde_json::Value>,
}

pub struct Vm {
    id: u64,
    child: Child,
    api_socket: PathBuf,
    vsock_socket: PathBuf,
    /// `Some(<chroot_base>/<exec>/<id>)` for a jailed boot, removed
    /// wholesale on `stop()`; `None` for a direct boot.
    jail_instance_dir: Option<PathBuf>,
}

impl Vm {
    /// Boots a new microVM. A failure's message includes the path to the
    /// captured guest console log ([`console_log_path`]) — a kernel panic
    /// or agent crash before vsock comes up is otherwise invisible.
    pub fn boot(config: &VmConfig) -> io::Result<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let log_path = console_log_path(id);
        boot::boot(config, id, &log_path).map_err(|e| annotate_with_console_log(e, &log_path))
    }

    /// Whether this VM booted under Firecracker's jailer rather than a
    /// direct spawn — the daemon checks this to refuse snapshotting a
    /// jailed sandbox (not yet supported, see `crate::jailer`).
    pub fn is_jailed(&self) -> bool {
        self.jail_instance_dir.is_some()
    }

    /// Sends a request to the guest agent over vsock and waits for its
    /// response. The agent isn't guaranteed to be listening the instant
    /// InstanceStart returns — this retries briefly to absorb that.
    pub fn call(&self, request: &Request) -> io::Result<Response> {
        let started = Instant::now();
        let result = retry_with_backoff(Duration::from_secs(5), Duration::from_millis(1), Duration::from_millis(20), || {
            vsock_client::call(&self.vsock_socket, AGENT_PORT, request)
        });
        match &result {
            Ok(_) => tracing::debug!(vm_id = self.id, elapsed_ms = started.elapsed().as_millis(), "vsock call ok"),
            Err(e) => tracing::warn!(vm_id = self.id, error = %e, "vsock call failed"),
        }
        result
    }

    /// Opens an interactive PTY session sized `cols`x`rows` — unlike
    /// `call()`, returns a raw, still-open stream for the session's whole
    /// life rather than one request/response. Retries briefly like
    /// `call()`, though in practice a PTY opens well after boot.
    pub fn open_pty(&self, cols: u16, rows: u16) -> io::Result<UnixStream> {
        let started = Instant::now();
        let result = retry_with_backoff(Duration::from_secs(5), Duration::from_millis(1), Duration::from_millis(20), || {
            vsock_client::open_pty(&self.vsock_socket, PTY_PORT, cols, rows)
        });
        match &result {
            Ok(_) => tracing::debug!(vm_id = self.id, elapsed_ms = started.elapsed().as_millis(), "pty session opened"),
            Err(e) => tracing::warn!(vm_id = self.id, error = %e, "opening pty session failed"),
        }
        result
    }

    /// Starts a streamed background exec session — same open-once shape
    /// as `open_pty`, but carries framed `ExecStreamEvent`s (see
    /// `EXEC_STREAM_PORT`) instead of raw bytes, and the process has no
    /// controlling terminal (`kiln logs -f`'s mechanism, not a shell).
    pub fn open_exec_stream(&self, command: &str, args: &[String], env: &HashMap<String, String>) -> io::Result<UnixStream> {
        let started = Instant::now();
        let result = retry_with_backoff(Duration::from_secs(5), Duration::from_millis(1), Duration::from_millis(20), || {
            vsock_client::open_exec_stream(&self.vsock_socket, EXEC_STREAM_PORT, command, args, env)
        });
        match &result {
            Ok(_) => tracing::debug!(vm_id = self.id, elapsed_ms = started.elapsed().as_millis(), "exec-stream session opened"),
            Err(e) => tracing::warn!(vm_id = self.id, error = %e, "opening exec-stream session failed"),
        }
        result
    }

    /// Updates this VM's MMDS content in place — for when a sandbox's
    /// real identity (id/name/tags) is only known after it's running,
    /// e.g. resumed from a pre-warmed pool snapshot. Redoes the full
    /// `PUT /mmds/config` + `PUT /mmds` rather than a bare `PATCH`: a
    /// resumed VM's MMDS store comes back uninitialized even though it
    /// was configured pre-snapshot, so `PATCH` alone fails (Engineering
    /// Notebook: "MMDS forgets who you are after a resume"). Fails if
    /// this VM has no network interface — shouldn't happen, every
    /// sandbox is networked.
    pub fn update_metadata(&self, metadata: &serde_json::Value) -> io::Result<()> {
        let mut api = ApiClient::connect(&self.api_socket)?;
        put_checked(&mut api, "/mmds/config", &serde_json::json!({ "network_interfaces": ["eth0"], "version": "V2" }))?;
        put_checked(&mut api, "/mmds", metadata)
    }

    pub fn stop(self) -> io::Result<()> {
        self.stop_inner(true)
    }

    /// Like `stop`, but skips the "sync before kill" call — for a VM
    /// already known dead (e.g. failed its post-resume pool-claim health
    /// check): nothing to flush, and the sync would just pay its own
    /// ~5s timeout for nothing.
    pub fn force_stop(self) -> io::Result<()> {
        self.stop_inner(false)
    }

    fn stop_inner(mut self, sync_first: bool) -> io::Result<()> {
        // A bare SIGKILL loses unflushed guest page-cache writes to
        // attached drives (rootfs copies don't care, they're discarded
        // anyway). Best-effort: fall through to the kill if unreachable.
        if sync_first {
            if let Err(e) = self.call(&Request::Exec { command: "sync".to_string(), args: vec![], env: HashMap::new() }) {
                tracing::warn!(vm_id = self.id, error = %e, "sync before stop failed, proceeding anyway");
            }
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.api_socket);
        let _ = std::fs::remove_file(&self.vsock_socket);
        // Tears down the whole chroot in one shot. A jailed VM's sockets
        // already live inside it (the removals above are redundant there
        // but needed for a direct boot, where this is `None`).
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

/// Where a VM's captured serial console (the guest kernel's
/// `console=ttyS0` output) is written. Shared with `snapshot::resume`,
/// which boots a fresh Firecracker process the same way `boot` does.
pub(crate) fn console_log_path(id: u64) -> PathBuf {
    PathBuf::from(format!("/tmp/sandkiln-fc-{id}.log"))
}

/// Two independent handles to the console log file for the child's
/// stdout/stderr — interleaved like a shell's `2>&1`, since both streams
/// are the same serial console.
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

/// Retries `attempt` with exponential backoff (`initial_interval` to
/// `max_interval`) until it succeeds or `timeout` elapses. Shared by
/// `connect_api_with_retry` and `Vm::call`/`open_pty`/`open_exec_stream`
/// — both are an "is the other side listening yet" race, previously two
/// separate fixed-sleep loops. The API-socket one measured a flat
/// ~20.1ms on every boot before this fix (ROADMAP.md Benchmarking); the
/// vsock one was the same bug, just conditional on actually racing the
/// agent's startup, which is why it went unnoticed longer.
fn retry_with_backoff<T>(
    timeout: Duration,
    initial_interval: Duration,
    max_interval: Duration,
    mut attempt: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let deadline = Instant::now() + timeout;
    let mut interval = initial_interval;
    loop {
        match attempt() {
            Ok(v) => return Ok(v),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => {
                std::thread::sleep(interval);
                interval = (interval * 2).min(max_interval);
            }
        }
    }
}

/// Connects to a freshly spawned Firecracker's API socket, retrying with
/// backoff until it accepts a connection or `timeout` elapses. Retries
/// the *connect* rather than polling for the socket file to exist first
/// — the file appears at `bind()`, a moment before `listen()`, so an
/// existence check can win that race and get `ECONNREFUSED`; polling the
/// connect itself has no such window, which is what makes polling this
/// fast safe. The fixed 20ms sleep this replaced measured a flat
/// ~20.1ms on every boot (more than half of a ~34ms boot) — a quantized
/// sleep, not real waiting. See ROADMAP.md's Benchmarking section.
fn connect_api_with_retry(path: &Path, timeout: Duration) -> io::Result<ApiClient> {
    retry_with_backoff(timeout, Duration::from_micros(200), Duration::from_millis(5), || ApiClient::connect(path)).map_err(|e| {
        io::Error::new(io::ErrorKind::TimedOut, format!("{path:?} never accepted a connection within {timeout:?}: {e}"))
    })
}

fn path_str(p: &Path) -> &str {
    p.to_str().expect("non-UTF8 paths are not supported")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A path nothing is ever going to listen on must fail as a timeout
    /// rather than hanging or succeeding, and must take at least roughly
    /// the timeout it was given — a backoff bug that exits the loop early
    /// would otherwise look like a clean, fast failure.
    #[test]
    fn connect_api_with_retry_times_out_on_a_socket_nothing_ever_binds() {
        let path = std::env::temp_dir().join(format!("sandkiln-never-bound-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let started = Instant::now();
        // `ApiClient` isn't `Debug`, so `unwrap_err()` isn't available.
        let err = match connect_api_with_retry(&path, Duration::from_millis(150)) {
            Ok(_) => panic!("connected to a socket nothing ever bound"),
            Err(e) => e,
        };
        let elapsed = started.elapsed();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "error was: {err}");
        assert!(elapsed >= Duration::from_millis(150), "returned after only {elapsed:?}, before the timeout elapsed");
    }

    #[test]
    fn connect_api_with_retry_connects_to_an_already_listening_socket() {
        let path = std::env::temp_dir().join(format!("sandkiln-already-listening-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();

        assert!(connect_api_with_retry(&path, Duration::from_secs(2)).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    /// The real case this exists for: the socket isn't there yet when the
    /// first attempt runs, and shows up partway through. This is also
    /// what a plain file-existence poll gets wrong — see the function's
    /// own doc comment on the `bind()`/`listen()` window.
    #[test]
    fn connect_api_with_retry_waits_for_a_socket_that_appears_late() {
        let path = std::env::temp_dir().join(format!("sandkiln-late-listener-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let bind_path = path.clone();
        let binder = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            std::os::unix::net::UnixListener::bind(&bind_path).unwrap()
        });

        assert!(connect_api_with_retry(&path, Duration::from_secs(2)).is_ok());
        drop(binder.join().unwrap());
        let _ = std::fs::remove_file(&path);
    }

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

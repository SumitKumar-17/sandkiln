use std::net::Ipv4Addr;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::Duration;

/// Firecracker jailer support: chroot, cgroup v2 limits, a dedicated
/// unprivileged uid/gid per VM. Presence of a `Config::jailer` (vs.
/// `None`) is what turns this on for the whole daemon — see
/// `Config::from_env`'s `SANDKILN_JAILER_ENABLED` handling.
///
/// Deliberately a daemon-operator setting, not a per-`POST /sandboxes`
/// request field: letting an API caller opt out of a security boundary
/// the operator turned on would defeat the point of turning it on. See
/// `sandkiln_vmm::jailer`'s module doc comment for what jailer actually
/// does and why it needs the `jailer` binary itself made setuid-root
/// (`SELF_HOSTING.md` documents the one-time setup).
pub struct JailerHostConfig {
    pub jailer_bin: PathBuf,
    pub chroot_base_dir: PathBuf,
    /// The uid/gid range dedicated to jailed VMs — see
    /// `sandkiln_vmm::jailer::JailerIdPool`'s doc comment for why this
    /// needs to be a range the host doesn't use for anything else.
    pub uid_gid_range: RangeInclusive<u32>,
}

pub struct Config {
    pub listen_addr: String,
    pub firecracker_bin: PathBuf,
    pub kernel_path: PathBuf,
    pub base_rootfs_path: PathBuf,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
    /// Upper bound on a per-sandbox `vcpu_count` override accepted via
    /// `POST /sandboxes` (see `routes_sandbox::CreateSandboxRequest`) —
    /// without a ceiling, a caller could ask for a VM sized to exhaust the
    /// host. Doesn't affect `vcpu_count` above, which is still what's used
    /// when a request doesn't override it, and is expected to be `<=`
    /// this (checked at startup, below).
    pub max_vcpu_count: u8,
    /// Same ceiling, for `mem_size_mib`.
    pub max_mem_size_mib: u32,
    pub bridge_name: String,
    pub bridge_gateway: Ipv4Addr,
    /// The host interface sandbox traffic gets NATed out through. `None`
    /// means "detect the default route interface at startup" — see
    /// `network::detect_uplink_iface`.
    pub uplink_iface: Option<String>,
    /// Must match what `scripts/create-tap-pool.sh` was run with — this
    /// is the daemon's max concurrent-sandbox-with-networking ceiling.
    pub tap_pool_prefix: String,
    pub tap_pool_size: u32,
    /// Bearer token required on every `/sandboxes*` request. `None` (the
    /// env var unset) disables auth entirely — fine for local dev, not
    /// for anything reachable beyond localhost.
    pub auth_token: Option<String>,
    /// Where persistent drives live. Deliberately not
    /// `std::env::temp_dir()` — that's where `create_sandbox` puts
    /// per-sandbox rootfs copies, which get deleted on sandbox stop.
    /// Drives are meant to outlive that, so they get their own directory.
    pub drives_dir: PathBuf,
    /// Where registered images live (see `sandkiln_vmm::image::ImageStore`
    /// and `routes_images`) — a caller-named, daemon-tracked rootfs a
    /// `POST /sandboxes` request can boot from instead of
    /// `base_rootfs_path`. Deliberately its own directory rather than
    /// reusing `drives_dir`: images and drives are different resource
    /// kinds (bootable rootfs vs. attachable block device) that happen to
    /// share a storage shape, and keeping them apart avoids an `<id>.ext4`
    /// collision between the two id namespaces.
    pub images_dir: PathBuf,
    /// How long a sandbox can go without any exec/read-file/write-file
    /// activity before the daemon stops it automatically — VM killed,
    /// network lease released, rootfs deleted, state gone for good (see
    /// `idle_reaper`). `None` — the env var unset, or set to `0` — disables
    /// this entirely: sandboxes run until explicitly stopped, today's
    /// behavior, unchanged unless a self-hosted instance opts in.
    ///
    /// See `auto_suspend_timeout`'s doc comment for how the two interact
    /// when both are configured.
    pub idle_timeout: Option<Duration>,
    /// How long a sandbox can go without activity before the daemon
    /// auto-suspends it instead of destroying it: pauses the microVM,
    /// snapshots it to disk (the same pause+snapshot path
    /// `POST /sandboxes/:id/snapshot` uses), and releases the VM process
    /// and its vcpu/memory — cheaper than staying booted, and resumable
    /// without a cold boot. The sandbox disappears from `GET /sandboxes`
    /// and reappears as a `Snapshot` (`Snapshot::source_sandbox_id` still
    /// points at the original sandbox id — see
    /// `routes_snapshot::list_snapshots`'s `?source_sandbox_id=` filter for
    /// how a caller finds the resulting snapshot). `None` — the env var
    /// unset, or set to `0` — disables this entirely, matching
    /// `idle_timeout`'s opt-in, no-silent-behavior-change pattern.
    ///
    /// When both this and `idle_timeout` are configured, this must be
    /// strictly less than `idle_timeout` (enforced in `from_env`, below).
    /// The policy: auto-suspend always gets first crack at an idle
    /// sandbox, and `idle_timeout` becomes a backstop rather than a
    /// competing timer — a sandbox that suspends successfully leaves
    /// `AppState::sandboxes` entirely (it's a `Snapshot` now), so
    /// `idle_timeout` never runs against it again; a sandbox whose
    /// auto-suspend keeps failing (e.g. a full disk, see
    /// `idle_reaper::reap_once`) keeps accruing idle time as an ordinary
    /// running sandbox and is eventually reclaimed by `idle_timeout`
    /// instead of running forever because its cheaper suspend path is
    /// broken. Requiring the strict ordering at startup, rather than
    /// letting an operator configure them the other way around, rules out
    /// a configuration where destroy would race ahead of suspend and make
    /// this setting silently pointless.
    pub auto_suspend_timeout: Option<Duration>,
    /// `SANDKILN_LOG_FORMAT=json` switches structured logging to one
    /// JSON object per line, for production log pipelines that expect to
    /// parse fields rather than a human-readable terminal format.
    pub log_format: LogFormat,
    /// How long `GET/POST/... /sandboxes/:id/preview/:port/*path` waits
    /// for the guest's dev server to respond before giving up with a 504.
    /// Deliberately generous compared to `exec`'s latency: a dev server
    /// can be slow to first-compile a page (webpack/vite cold start).
    pub preview_timeout: Duration,
    /// `None` (the default) — `SANDKILN_JAILER_ENABLED` unset or falsy —
    /// keeps today's direct Firecracker spawn. See `JailerHostConfig`.
    pub jailer: Option<JailerHostConfig>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogFormat {
    Pretty,
    Json,
}

impl LogFormat {
    fn from_env() -> Self {
        Self::parse(std::env::var("SANDKILN_LOG_FORMAT").ok().as_deref())
    }

    fn parse(value: Option<&str>) -> Self {
        match value {
            Some(v) if v.eq_ignore_ascii_case("json") => LogFormat::Json,
            _ => LogFormat::Pretty,
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let vcpu_count: u8 = env_or("SANDKILN_VCPU_COUNT", "2").parse().expect("SANDKILN_VCPU_COUNT must be a number");
        let mem_size_mib: u32 =
            env_or("SANDKILN_MEM_SIZE_MIB", "512").parse().expect("SANDKILN_MEM_SIZE_MIB must be a number");
        // Defaults chosen generously enough not to surprise a self-hoster
        // who never touches these — 16 vCPUs / 16 GiB is well above
        // anything a single sandbox plausibly needs — while still being an
        // actual ceiling rather than "unbounded unless you opt in", since
        // the whole point is closing a resource-exhaustion gap by default,
        // not just when an operator remembers to configure one.
        let max_vcpu_count: u8 =
            env_or("SANDKILN_MAX_VCPU_COUNT", "16").parse().expect("SANDKILN_MAX_VCPU_COUNT must be a number");
        let max_mem_size_mib: u32 = env_or("SANDKILN_MAX_MEM_SIZE_MIB", "16384")
            .parse()
            .expect("SANDKILN_MAX_MEM_SIZE_MIB must be a number");
        assert!(
            vcpu_count <= max_vcpu_count,
            "SANDKILN_VCPU_COUNT ({vcpu_count}) exceeds SANDKILN_MAX_VCPU_COUNT ({max_vcpu_count}) — the daemon's own default would be rejected"
        );
        assert!(
            mem_size_mib <= max_mem_size_mib,
            "SANDKILN_MEM_SIZE_MIB ({mem_size_mib}) exceeds SANDKILN_MAX_MEM_SIZE_MIB ({max_mem_size_mib}) — the daemon's own default would be rejected"
        );

        let idle_timeout = parse_timeout_secs_env("SANDKILN_IDLE_TIMEOUT_SECS");
        let auto_suspend_timeout = parse_timeout_secs_env("SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS");
        if let Err(message) = check_suspend_precedes_destroy(auto_suspend_timeout, idle_timeout) {
            panic!("{message}");
        }

        Self {
            listen_addr: env_or("SANDKILN_LISTEN_ADDR", "127.0.0.1:7777"),
            firecracker_bin: expand_home(&env_or("SANDKILN_FIRECRACKER_BIN", "~/sandkiln-tools/bin/firecracker")),
            kernel_path: expand_home(&env_or("SANDKILN_KERNEL_PATH", "~/sandkiln-tools/images/vmlinux-5.10.223")),
            base_rootfs_path: expand_home(&env_or("SANDKILN_BASE_ROOTFS", "~/sandkiln-tools/images/ubuntu-22.04.ext4")),
            vcpu_count,
            mem_size_mib,
            max_vcpu_count,
            max_mem_size_mib,
            bridge_name: env_or("SANDKILN_BRIDGE_NAME", "sktapbr0"),
            bridge_gateway: env_or("SANDKILN_BRIDGE_GATEWAY", "172.16.0.1")
                .parse()
                .expect("SANDKILN_BRIDGE_GATEWAY must be an IPv4 address"),
            uplink_iface: std::env::var("SANDKILN_UPLINK_IFACE").ok(),
            tap_pool_prefix: env_or("SANDKILN_TAP_POOL_PREFIX", "sktap"),
            tap_pool_size: env_or("SANDKILN_TAP_POOL_SIZE", "32").parse().expect("SANDKILN_TAP_POOL_SIZE must be a number"),
            auth_token: std::env::var("SANDKILN_AUTH_TOKEN").ok(),
            drives_dir: expand_home(&env_or("SANDKILN_DRIVES_DIR", "~/sandkiln-tools/drives")),
            images_dir: expand_home(&env_or("SANDKILN_IMAGES_DIR", "~/sandkiln-tools/images-registered")),
            idle_timeout,
            auto_suspend_timeout,
            log_format: LogFormat::from_env(),
            preview_timeout: Duration::from_secs(
                env_or("SANDKILN_PREVIEW_TIMEOUT_SECS", "30").parse().expect("SANDKILN_PREVIEW_TIMEOUT_SECS must be a number"),
            ),
            jailer: jailer_config_from_env(),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Shared parsing for `SANDKILN_IDLE_TIMEOUT_SECS` and
/// `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`: unset or `0` both mean "disabled"
/// rather than the env var needing to be entirely absent to opt out, so a
/// self-hoster can flip one off in a shared `.env` file by setting it to
/// `0` instead of deleting the line.
fn parse_timeout_secs_env(key: &str) -> Option<Duration> {
    std::env::var(key)
        .ok()
        .map(|v| v.parse::<u64>().unwrap_or_else(|_| panic!("{key} must be a number")))
        .filter(|secs| *secs > 0)
        .map(Duration::from_secs)
}

fn parse_bool_env(value: Option<&str>) -> bool {
    matches!(value, Some(v) if v.eq_ignore_ascii_case("true") || v == "1")
}

/// The uid/gid range dedicated to jailed VMs: `base..=(base + size - 1)`.
/// A `size` of `0` would produce an empty (and thus useless) range —
/// callers should treat that as a configuration error rather than a
/// silently-disabled pool, so this doesn't special-case it away.
fn jailer_uid_gid_range(base: u32, size: u32) -> RangeInclusive<u32> {
    base..=(base + size.saturating_sub(1))
}

fn jailer_config_from_env() -> Option<JailerHostConfig> {
    if !parse_bool_env(std::env::var("SANDKILN_JAILER_ENABLED").ok().as_deref()) {
        return None;
    }
    let uid_gid_base: u32 =
        env_or("SANDKILN_JAILER_UID_GID_BASE", "600000").parse().expect("SANDKILN_JAILER_UID_GID_BASE must be a number");
    let pool_size: u32 =
        env_or("SANDKILN_JAILER_POOL_SIZE", "32").parse().expect("SANDKILN_JAILER_POOL_SIZE must be a number");
    assert!(pool_size > 0, "SANDKILN_JAILER_POOL_SIZE must be at least 1 when jailer is enabled");
    Some(JailerHostConfig {
        jailer_bin: expand_home(&env_or("SANDKILN_JAILER_BIN", "~/sandkiln-tools/bin/jailer")),
        chroot_base_dir: expand_home(&env_or("SANDKILN_JAILER_CHROOT_BASE_DIR", "~/sandkiln-tools/jail")),
        uid_gid_range: jailer_uid_gid_range(uid_gid_base, pool_size),
    })
}

/// Pure validation behind the `auto_suspend_timeout`/`idle_timeout`
/// interaction documented on `Config::auto_suspend_timeout` — pulled out
/// of `from_env` so the policy is directly testable without going through
/// process-global env vars, same as `jailer_uid_gid_range` above. Passing
/// (both unset, only one set, or `auto_suspend_timeout < idle_timeout`)
/// returns `Ok`; the one invalid combination — auto-suspend configured at
/// or past the destroy threshold, which would make it a race `idle_timeout`
/// can win instead of a guaranteed-first backstop relationship — returns an
/// `Err` describing why.
fn check_suspend_precedes_destroy(auto_suspend_timeout: Option<Duration>, idle_timeout: Option<Duration>) -> Result<(), String> {
    match (auto_suspend_timeout, idle_timeout) {
        (Some(suspend), Some(destroy)) if suspend >= destroy => Err(format!(
            "SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS ({}) must be strictly less than SANDKILN_IDLE_TIMEOUT_SECS ({}) when both \
             are set — auto-suspend needs to reach every idle sandbox before the destroy timeout would tear it down instead \
             of suspending it; see `Config::auto_suspend_timeout`'s doc comment",
            suspend.as_secs(),
            destroy.as_secs()
        )),
        _ => Ok(()),
    }
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").expect("HOME must be set to expand ~")).join(rest),
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_format_defaults_to_pretty_when_unset() {
        assert_eq!(LogFormat::parse(None), LogFormat::Pretty);
    }

    #[test]
    fn log_format_parses_json_case_insensitively() {
        assert_eq!(LogFormat::parse(Some("json")), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some("JSON")), LogFormat::Json);
        assert_eq!(LogFormat::parse(Some("Json")), LogFormat::Json);
    }

    #[test]
    fn log_format_falls_back_to_pretty_for_anything_else() {
        assert_eq!(LogFormat::parse(Some("pretty")), LogFormat::Pretty);
        assert_eq!(LogFormat::parse(Some("")), LogFormat::Pretty);
        assert_eq!(LogFormat::parse(Some("yaml")), LogFormat::Pretty);
    }

    #[test]
    fn parse_bool_env_accepts_true_and_1_case_insensitively() {
        assert!(parse_bool_env(Some("true")));
        assert!(parse_bool_env(Some("True")));
        assert!(parse_bool_env(Some("TRUE")));
        assert!(parse_bool_env(Some("1")));
    }

    #[test]
    fn parse_bool_env_rejects_everything_else_including_unset() {
        assert!(!parse_bool_env(None));
        assert!(!parse_bool_env(Some("false")));
        assert!(!parse_bool_env(Some("0")));
        assert!(!parse_bool_env(Some("yes")));
        assert!(!parse_bool_env(Some("")));
    }

    #[test]
    fn jailer_uid_gid_range_covers_exactly_size_ids_starting_at_base() {
        let range = jailer_uid_gid_range(600000, 32);
        assert_eq!(*range.start(), 600000);
        assert_eq!(*range.end(), 600031);
        assert_eq!(range.count(), 32);
    }

    #[test]
    fn jailer_uid_gid_range_of_size_one_is_a_single_id() {
        let range = jailer_uid_gid_range(700000, 1);
        assert_eq!(*range.start(), 700000);
        assert_eq!(*range.end(), 700000);
    }

    #[test]
    fn suspend_precedes_destroy_ok_when_neither_is_set() {
        assert!(check_suspend_precedes_destroy(None, None).is_ok());
    }

    #[test]
    fn suspend_precedes_destroy_ok_when_only_one_is_set() {
        assert!(check_suspend_precedes_destroy(Some(Duration::from_secs(60)), None).is_ok());
        assert!(check_suspend_precedes_destroy(None, Some(Duration::from_secs(60))).is_ok());
    }

    #[test]
    fn suspend_precedes_destroy_ok_when_suspend_is_strictly_shorter() {
        assert!(check_suspend_precedes_destroy(Some(Duration::from_secs(60)), Some(Duration::from_secs(300))).is_ok());
    }

    #[test]
    fn suspend_precedes_destroy_rejects_suspend_equal_to_destroy() {
        let err = check_suspend_precedes_destroy(Some(Duration::from_secs(60)), Some(Duration::from_secs(60))).unwrap_err();
        assert!(err.contains("SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS"));
        assert!(err.contains("SANDKILN_IDLE_TIMEOUT_SECS"));
    }

    #[test]
    fn suspend_precedes_destroy_rejects_suspend_longer_than_destroy() {
        assert!(check_suspend_precedes_destroy(Some(Duration::from_secs(600)), Some(Duration::from_secs(60))).is_err());
    }
}

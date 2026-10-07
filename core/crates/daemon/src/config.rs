use std::net::Ipv4Addr;
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::time::Duration;

/// Firecracker jailer support: chroot, cgroup v2 limits, a dedicated
/// unprivileged uid/gid per VM. `Config::jailer: Some(_)` turns this on
/// for the whole daemon (`SANDKILN_JAILER_ENABLED`). Daemon-operator
/// setting, not a per-request field — letting a caller opt out would
/// defeat a boundary the operator turned on. See `sandkiln_vmm::jailer`
/// for the mechanism and why `jailer` itself needs setuid-root
/// (`SELF_HOSTING.md`).
pub struct JailerHostConfig {
    pub jailer_bin: PathBuf,
    pub chroot_base_dir: PathBuf,
    /// Uid/gid range dedicated to jailed VMs — must not overlap anything
    /// else on the host (`sandkiln_vmm::jailer::JailerIdPool`).
    pub uid_gid_range: RangeInclusive<u32>,
}

pub struct Config {
    pub listen_addr: String,
    pub firecracker_bin: PathBuf,
    pub kernel_path: PathBuf,
    pub base_rootfs_path: PathBuf,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
    /// Ceiling on a per-sandbox `vcpu_count` override (`POST /sandboxes`)
    /// — without it a caller could size a VM to exhaust the host.
    /// `vcpu_count` above must be `<=` this (checked at startup).
    pub max_vcpu_count: u8,
    /// Same ceiling, for `mem_size_mib`.
    pub max_mem_size_mib: u32,
    pub bridge_name: String,
    pub bridge_gateway: Ipv4Addr,
    /// Host interface sandbox traffic NATs out through. `None` = detect
    /// the default route at startup (`network::detect_uplink_iface`).
    pub uplink_iface: Option<String>,
    /// Must match `scripts/host-setup/create-tap-pool.sh`'s own run — the
    /// daemon's max concurrent-sandbox-with-networking ceiling.
    pub tap_pool_prefix: String,
    pub tap_pool_size: u32,
    /// Bearer token for `/sandboxes*`. `None` (unset) disables auth
    /// entirely — fine for local dev, not beyond localhost.
    pub auth_token: Option<String>,
    /// Persistent drives' home. Not `std::env::temp_dir()` — that's
    /// per-sandbox rootfs copies, deleted on stop; drives outlive that.
    pub drives_dir: PathBuf,
    /// Registered images' home (`sandkiln_vmm::image::ImageStore`,
    /// `routes_images`) — a named rootfs `POST /sandboxes` can boot from
    /// instead of `base_rootfs_path`. Own directory, not `drives_dir`:
    /// different resource kinds sharing a storage shape would otherwise
    /// collide on `<id>.ext4` across the two id namespaces.
    pub images_dir: PathBuf,
    /// Durable sandbox-history database (`sandkiln_store::HistoryStore`)
    /// — its own file, a third unrelated resource kind.
    pub history_db_path: PathBuf,
    /// Idle time before the daemon destroys a sandbox outright (VM
    /// killed, lease released, rootfs deleted — see `idle_reaper`).
    /// `None`/`0` disables it; sandboxes then run until explicitly
    /// stopped. See `auto_suspend_timeout` for how the two interact.
    pub idle_timeout: Option<Duration>,
    /// Idle time before the daemon auto-suspends instead of destroying:
    /// pause + snapshot (same path as `POST /sandboxes/:id/snapshot`),
    /// releasing the VM process/vcpu/memory while staying resumable
    /// without a cold boot. The sandbox becomes a `Snapshot`
    /// (`source_sandbox_id` still points at the original id). `None`/`0`
    /// disables it, same opt-in pattern as `idle_timeout`.
    ///
    /// Must be strictly less than `idle_timeout` when both are set
    /// (enforced in `from_env`): auto-suspend gets first crack at an idle
    /// sandbox, `idle_timeout` is the backstop for when suspend keeps
    /// failing (e.g. a full disk), not a competing timer. A successful
    /// suspend removes the sandbox from `AppState::sandboxes` entirely,
    /// so `idle_timeout` never runs against it again. Enforcing the order
    /// at startup rules out a config where destroy could race ahead and
    /// make this setting silently pointless.
    pub auto_suspend_timeout: Option<Duration>,
    /// Idle time before a *held snapshot* (any origin) gets its
    /// `state.snap`/`mem.bin` moved from `snapshots_root()` to
    /// `archive_dir` — the archive tier of running → suspended →
    /// archived. `None`/`0` disables it; independent of
    /// `auto_suspend_timeout`/`idle_timeout` (a snapshot's own age, not a
    /// live sandbox's idle time — applies whether auto-suspend is
    /// configured or not). A snapshot with a live fork is never archived,
    /// same exclusion resume/delete already apply.
    ///
    /// **Only `state.snap`/`mem.bin` move — never the rootfs file** (see
    /// `crate::snapshot::move_snapshot_files`). Partial, not complete:
    /// `mem.bin` alone is often comparable to or larger than the rootfs
    /// copy, so this still meaningfully cuts hot-storage usage. Also not
    /// `ROADMAP.md`'s originally-sketched remote-storage tier — still a
    /// local path, just a separately configured one.
    pub archive_timeout: Option<Duration>,
    /// Archived snapshots' home — meaningful only with `archive_timeout`
    /// set, but always has a value (same convention as
    /// `drives_dir`/`images_dir`) rather than a nested `Option`.
    pub archive_dir: PathBuf,
    /// `SANDKILN_LOG_FORMAT=json` switches to one JSON object per line,
    /// for pipelines parsing fields rather than reading a terminal.
    pub log_format: LogFormat,
    /// How long `.../preview/:port/*path` waits before a `504`.
    /// Generous relative to `exec` — a dev server can be slow to
    /// first-compile (webpack/vite cold start).
    pub preview_timeout: Duration,
    /// `None` (default, `SANDKILN_JAILER_ENABLED` unset/falsy) keeps
    /// direct Firecracker spawn. See `JailerHostConfig`.
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
        // Default ceiling (16 vCPU/16GiB) is generous but real — closes
        // the resource-exhaustion gap by default, not only when an
        // operator remembers to configure one.
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
        let archive_timeout = parse_timeout_secs_env("SANDKILN_ARCHIVE_TIMEOUT_SECS");

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
            history_db_path: expand_home(&env_or("SANDKILN_HISTORY_DB_PATH", "~/sandkiln-tools/history.db")),
            images_dir: expand_home(&env_or("SANDKILN_IMAGES_DIR", "~/sandkiln-tools/images-registered")),
            idle_timeout,
            auto_suspend_timeout,
            archive_timeout,
            archive_dir: expand_home(&env_or("SANDKILN_ARCHIVE_DIR", "~/sandkiln-tools/archive")),
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

/// Shared by the three `*_TIMEOUT_SECS` vars: unset or `0` both mean
/// "disabled," so a self-hoster can flip one off in a shared `.env` by
/// setting `0` instead of deleting the line.
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

/// `base..=(base + size - 1)`. `size: 0` produces an empty range on
/// purpose — a config error to surface, not a silently-disabled pool.
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

/// The `auto_suspend_timeout`/`idle_timeout` ordering check, pulled out
/// of `from_env` so it's testable without process-global env vars. `Err`
/// only when auto-suspend is at or past the destroy threshold — the one
/// combination that breaks the guaranteed-first-backstop relationship.
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

mod settings;
mod source;
pub mod cli;

pub use settings::{reference_markdown, SETTINGS};
pub use source::Origin;

use source::Resolver;
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
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
    /// Where the effective values came from, for `--print-config`.
    pub origins: std::collections::BTreeMap<&'static str, Origin>,
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
    /// Durable daemon state (snapshots, retired checkpoints). Not the OS
    /// temp dir: a tmp cleaner would silently destroy "durable" snapshots.
    pub state_dir: PathBuf,
    /// Per-sandbox writable rootfs files. Put this on the same reflink-capable
    /// filesystem (XFS/Btrfs) as the base image to make clones instant; the
    /// `rootfs_clone_total{method}` metric shows whether that worked.
    pub rootfs_dir: PathBuf,
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
    /// (enforced in `Config::resolve`): auto-suspend gets first crack at an idle
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
    /// `crate::snapshots::held::move_snapshot_files`). Partial, not complete:
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
    fn parse(value: Option<&str>) -> Self {
        match value {
            Some(v) if v.eq_ignore_ascii_case("json") => LogFormat::Json,
            _ => LogFormat::Pretty,
        }
    }
}

/// Everything startup needs to report: the config, plus non-fatal findings.
pub struct Loaded {
    pub config: Config,
    pub file: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// Why startup configuration failed. Every problem found is listed, not just
/// the first, so one attempt is enough to fix a bad file.
#[derive(Debug)]
pub struct ConfigError(pub Vec<String>);

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "invalid configuration:")?;
        for e in &self.0 {
            writeln!(f, "  - {e}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigError {}

/// Where the config file is looked for when `--config` is not given:
/// `$SANDKILN_CONFIG`, then `./sandkiln.toml`, then
/// `$XDG_CONFIG_HOME/sandkiln/sandkiln.toml` (default `~/.config`).
pub fn discover_file(env: &HashMap<String, String>) -> Option<PathBuf> {
    if let Some(p) = env.get("SANDKILN_CONFIG").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let local = PathBuf::from("sandkiln.toml");
    if local.is_file() {
        return Some(local);
    }
    let xdg = env
        .get("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env.get("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let candidate = xdg.join("sandkiln").join("sandkiln.toml");
    candidate.is_file().then_some(candidate)
}

impl Config {
    /// Loads from the process environment and the discovered or given file.
    pub fn load(explicit_file: Option<&Path>) -> Result<Loaded, ConfigError> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let file = match explicit_file.map(Path::to_path_buf).or_else(|| discover_file(&env)) {
            None => None,
            Some(path) => {
                let contents = std::fs::read_to_string(&path)
                    .map_err(|e| ConfigError(vec![format!("cannot read config file {}: {e}", path.display())]))?;
                #[cfg(unix)]
                let insecure = {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::metadata(&path).map(|m| m.permissions().mode() & 0o077 != 0).unwrap_or(false)
                };
                #[cfg(not(unix))]
                let insecure = false;
                Some((path, contents, insecure))
            }
        };
        Self::resolve(env, file)
    }

    /// Pure resolution: no process state, so it is unit-testable.
    pub fn resolve(env: HashMap<String, String>, file: Option<(PathBuf, String, bool)>) -> Result<Loaded, ConfigError> {
        let mut warnings = Vec::new();
        let mut file_values = Default::default();
        let mut file_path = None;
        if let Some((path, contents, insecure)) = file {
            file_values = source::parse_file(&contents)
                .map_err(|errs| ConfigError(errs.into_iter().map(|e| format!("{}: {e}", path.display())).collect()))?;
            if insecure && file_values.contains_key("auth_token") {
                warnings.push(format!(
                    "{} contains auth_token but is readable by other users; run: chmod 600 {}",
                    path.display(),
                    path.display()
                ));
            }
            file_path = Some(path);
        }

        let mut r = Resolver::new(env, file_values);
        let vcpu_count: u8 = r.number("vcpu_count");
        let mem_size_mib: u32 = r.number("mem_size_mib");
        let max_vcpu_count: u8 = r.number("max_vcpu_count");
        let max_mem_size_mib: u32 = r.number("max_mem_size_mib");
        if vcpu_count > max_vcpu_count {
            r.error(format!("vcpu_count ({vcpu_count}) exceeds max_vcpu_count ({max_vcpu_count}): the daemon's own default would be rejected"));
        }
        if mem_size_mib > max_mem_size_mib {
            r.error(format!("mem_size_mib ({mem_size_mib}) exceeds max_mem_size_mib ({max_mem_size_mib}): the daemon's own default would be rejected"));
        }

        let idle_timeout = r.opt_secs("idle_timeout_secs");
        let auto_suspend_timeout = r.opt_secs("auto_suspend_timeout_secs");
        if let Err(message) = check_suspend_precedes_destroy(auto_suspend_timeout, idle_timeout) {
            r.error(message);
        }
        let archive_timeout = r.opt_secs("archive_timeout_secs");

        let bridge_gateway = match r.string("bridge_gateway").parse::<Ipv4Addr>() {
            Ok(ip) => ip,
            Err(_) => {
                r.error("bridge_gateway must be an IPv4 address".to_string());
                Ipv4Addr::UNSPECIFIED
            }
        };
        let state_dir = r.path("state_dir");
        let rootfs_dir = r.opt_path("rootfs_dir").unwrap_or_else(|| state_dir.join("rootfs"));

        let jailer = if r.flag("jailer_enabled") {
            let base: u32 = r.number("jailer_uid_gid_base");
            let pool_size: u32 = r.number("jailer_pool_size");
            if pool_size == 0 {
                r.error("jailer_pool_size must be at least 1 when the jailer is enabled".to_string());
            }
            Some(JailerHostConfig {
                jailer_bin: r.path("jailer_bin"),
                chroot_base_dir: r.path("jailer_chroot_base_dir"),
                uid_gid_range: jailer_uid_gid_range(base, pool_size),
            })
        } else {
            None
        };

        let config = Config {
            listen_addr: r.string("listen_addr"),
            firecracker_bin: r.path("firecracker_bin"),
            kernel_path: r.path("kernel_path"),
            base_rootfs_path: r.path("base_rootfs"),
            vcpu_count,
            mem_size_mib,
            max_vcpu_count,
            max_mem_size_mib,
            bridge_name: r.string("bridge_name"),
            bridge_gateway,
            uplink_iface: r.opt_string("uplink_iface"),
            tap_pool_prefix: r.string("tap_pool_prefix"),
            tap_pool_size: r.number("tap_pool_size"),
            auth_token: r.opt_string("auth_token"),
            drives_dir: r.path("drives_dir"),
            state_dir,
            rootfs_dir,
            images_dir: r.path("images_dir"),
            history_db_path: r.path("history_db_path"),
            idle_timeout,
            auto_suspend_timeout,
            archive_timeout,
            archive_dir: r.path("archive_dir"),
            log_format: LogFormat::parse(r.opt_string("log_format").as_deref()),
            preview_timeout: Duration::from_secs(r.number("preview_timeout_secs")),
            jailer,
            origins: Default::default(),
        };
        if !r.errors.is_empty() {
            return Err(ConfigError(r.errors));
        }
        let origins = r.origins;
        Ok(Loaded { config: Config { origins, ..config }, file: file_path, warnings })
    }

    /// Effective settings as `key = value  # origin`, secrets redacted.
    pub fn render_effective(&self, file: Option<&Path>) -> String {
        let mut out = match file {
            Some(p) => format!("# config file: {}\n", p.display()),
            None => "# config file: none (environment and defaults only)\n".to_string(),
        };
        for s in SETTINGS {
            let value = self.display_value(s.key);
            let origin = self.origins.get(s.key).copied().unwrap_or(Origin::Default);
            let shown = if s.secret && value.is_some() { "\"<redacted>\"".to_string() } else { value.unwrap_or_else(|| "unset".into()) };
            out.push_str(&format!("{} = {}  # {}\n", s.key, shown, origin.label()));
        }
        out
    }

    fn display_value(&self, key: &str) -> Option<String> {
        let p = |p: &PathBuf| Some(p.display().to_string());
        let secs = |d: &Option<Duration>| d.map(|d| d.as_secs().to_string());
        match key {
            "listen_addr" => Some(self.listen_addr.clone()),
            "auth_token" => self.auth_token.clone(),
            "log_format" => Some(format!("{:?}", self.log_format).to_lowercase()),
            "preview_timeout_secs" => Some(self.preview_timeout.as_secs().to_string()),
            "firecracker_bin" => p(&self.firecracker_bin),
            "kernel_path" => p(&self.kernel_path),
            "base_rootfs" => p(&self.base_rootfs_path),
            "vcpu_count" => Some(self.vcpu_count.to_string()),
            "mem_size_mib" => Some(self.mem_size_mib.to_string()),
            "max_vcpu_count" => Some(self.max_vcpu_count.to_string()),
            "max_mem_size_mib" => Some(self.max_mem_size_mib.to_string()),
            "state_dir" => p(&self.state_dir),
            "rootfs_dir" => p(&self.rootfs_dir),
            "drives_dir" => p(&self.drives_dir),
            "images_dir" => p(&self.images_dir),
            "history_db_path" => p(&self.history_db_path),
            "archive_dir" => p(&self.archive_dir),
            "bridge_name" => Some(self.bridge_name.clone()),
            "bridge_gateway" => Some(self.bridge_gateway.to_string()),
            "uplink_iface" => self.uplink_iface.clone(),
            "tap_pool_prefix" => Some(self.tap_pool_prefix.clone()),
            "tap_pool_size" => Some(self.tap_pool_size.to_string()),
            "idle_timeout_secs" => secs(&self.idle_timeout),
            "auto_suspend_timeout_secs" => secs(&self.auto_suspend_timeout),
            "archive_timeout_secs" => secs(&self.archive_timeout),
            "jailer_enabled" => Some(self.jailer.is_some().to_string()),
            "jailer_bin" => self.jailer.as_ref().and_then(|j| p(&j.jailer_bin)),
            "jailer_chroot_base_dir" => self.jailer.as_ref().and_then(|j| p(&j.chroot_base_dir)),
            "jailer_uid_gid_base" => self.jailer.as_ref().map(|j| j.uid_gid_range.start().to_string()),
            "jailer_pool_size" => self.jailer.as_ref().map(|j| j.uid_gid_range.clone().count().to_string()),
            _ => None,
        }
    }
}

/// `base..=(base + size - 1)`. `size: 0` produces an empty range on
/// purpose: a config error to surface, not a silently-disabled pool.
fn jailer_uid_gid_range(base: u32, size: u32) -> RangeInclusive<u32> {
    base..=(base + size.saturating_sub(1))
}

/// The `auto_suspend_timeout`/`idle_timeout` ordering check. `Err` only when
/// auto-suspend is at or past the destroy threshold: the one combination
/// that breaks the guaranteed-first-backstop relationship.
fn check_suspend_precedes_destroy(auto_suspend_timeout: Option<Duration>, idle_timeout: Option<Duration>) -> Result<(), String> {
    match (auto_suspend_timeout, idle_timeout) {
        (Some(suspend), Some(destroy)) if suspend >= destroy => Err(format!(
            "auto_suspend_timeout_secs ({}) must be strictly less than idle_timeout_secs ({}) when both are set: \
             auto-suspend has to reach every idle sandbox before the destroy timeout tears it down instead",
            suspend.as_secs(),
            destroy.as_secs()
        )),
        _ => Ok(()),
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
        assert!(err.contains("auto_suspend_timeout_secs"));
        assert!(err.contains("idle_timeout_secs"));
    }

    #[test]
    fn suspend_precedes_destroy_rejects_suspend_longer_than_destroy() {
        assert!(check_suspend_precedes_destroy(Some(Duration::from_secs(600)), Some(Duration::from_secs(60))).is_err());
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        let mut m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        m.entry("HOME".into()).or_insert_with(|| "/home/u".into());
        m
    }

    #[test]
    fn defaults_alone_produce_a_valid_config() {
        let loaded = Config::resolve(env(&[]), None).unwrap();
        assert_eq!(loaded.config.listen_addr, "127.0.0.1:7777");
        assert_eq!(loaded.config.rootfs_dir, PathBuf::from("/home/u/sandkiln-tools/state/rootfs"));
        assert!(loaded.config.jailer.is_none() && loaded.config.idle_timeout.is_none());
    }

    #[test]
    fn file_values_apply_and_env_overrides_them() {
        let file = Some((PathBuf::from("/etc/sandkiln.toml"), "vcpu_count = 4\nrootfs_dir = \"/mnt/xfs/rootfs\"\n".to_string(), false));
        let c = Config::resolve(env(&[("SANDKILN_VCPU_COUNT", "6")]), file).unwrap().config;
        assert_eq!(c.vcpu_count, 6);
        assert_eq!(c.rootfs_dir, PathBuf::from("/mnt/xfs/rootfs"));
        assert_eq!(c.origins["vcpu_count"], Origin::Env);
        assert_eq!(c.origins["rootfs_dir"], Origin::File);
    }

    #[test]
    fn all_problems_are_reported_together() {
        let e = Config::resolve(env(&[("SANDKILN_VCPU_COUNT", "99"), ("SANDKILN_BRIDGE_GATEWAY", "nope"), ("SANDKILN_TAP_POOL_SIZE", "x")]), None)
            .err()
            .unwrap();
        assert_eq!(e.0.len(), 3, "{e}");
    }

    #[test]
    fn unknown_file_key_fails_with_the_path_in_the_message() {
        let file = Some((PathBuf::from("/etc/sandkiln.toml"), "vcpus = 2\n".to_string(), false));
        let e = Config::resolve(env(&[]), file).err().unwrap();
        assert!(e.0[0].starts_with("/etc/sandkiln.toml:") && e.0[0].contains("unknown key `vcpus`"), "{e}");
    }

    #[test]
    fn insecure_file_with_a_token_warns_but_still_loads() {
        let file = Some((PathBuf::from("/etc/sandkiln.toml"), "auth_token = \"s3cret\"\n".to_string(), true));
        let loaded = Config::resolve(env(&[]), file).unwrap();
        assert!(loaded.warnings[0].contains("chmod 600"));
    }

    #[test]
    fn jailer_settings_only_matter_when_enabled() {
        let c = Config::resolve(env(&[("SANDKILN_JAILER_ENABLED", "true"), ("SANDKILN_JAILER_POOL_SIZE", "0")]), None);
        assert!(c.err().unwrap().0[0].contains("jailer_pool_size"));
        let c = Config::resolve(env(&[("SANDKILN_JAILER_ENABLED", "1")]), None).unwrap().config;
        assert_eq!(c.jailer.unwrap().uid_gid_range.count(), 32);
    }

    #[test]
    fn effective_config_redacts_the_token_and_shows_origins() {
        let c = Config::resolve(env(&[("SANDKILN_AUTH_TOKEN", "hunter2")]), None).unwrap().config;
        let out = c.render_effective(None);
        assert!(!out.contains("hunter2"));
        assert!(out.contains("auth_token = \"<redacted>\"  # env"));
        assert!(out.contains("listen_addr = 127.0.0.1:7777  # default"));
    }

    #[test]
    fn every_registered_setting_has_a_rendered_value_or_is_legitimately_unset() {
        let c = Config::resolve(env(&[("SANDKILN_JAILER_ENABLED", "true")]), None).unwrap().config;
        for s in SETTINGS {
            if s.default.is_some() {
                assert!(c.display_value(s.key).is_some(), "{} has a default but no display value", s.key);
            }
        }
    }

    const EXAMPLE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../sandkiln.example.toml"));

    /// The shipped example must document every setting, and uncommenting all
    /// of it must still be a valid configuration: it cannot drift from the code.
    #[test]
    fn example_file_covers_every_setting_and_is_valid_when_fully_uncommented() {
        let uncommented: String = EXAMPLE
            .lines()
            .map(|l| l.strip_prefix("# ").filter(|rest| rest.contains(" = ")).unwrap_or(l))
            .map(|l| format!("{l}\n"))
            .collect();
        for s in SETTINGS {
            assert!(uncommented.contains(&format!("\n{} = ", s.key)), "sandkiln.example.toml is missing `{}`", s.key);
        }
        let file = Some((PathBuf::from("sandkiln.example.toml"), uncommented, false));
        Config::resolve(env(&[]), file).unwrap_or_else(|e| panic!("{e}"));
    }
}

use std::net::Ipv4Addr;
use std::path::PathBuf;

pub struct Config {
    pub listen_addr: String,
    pub firecracker_bin: PathBuf,
    pub kernel_path: PathBuf,
    pub base_rootfs_path: PathBuf,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
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
    /// `SANDKILN_LOG_FORMAT=json` switches structured logging to one
    /// JSON object per line, for production log pipelines that expect to
    /// parse fields rather than a human-readable terminal format.
    pub log_format: LogFormat,
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
        Self {
            listen_addr: env_or("SANDKILN_LISTEN_ADDR", "127.0.0.1:7777"),
            firecracker_bin: expand_home(&env_or("SANDKILN_FIRECRACKER_BIN", "~/sandkiln-tools/bin/firecracker")),
            kernel_path: expand_home(&env_or("SANDKILN_KERNEL_PATH", "~/sandkiln-tools/images/vmlinux-5.10.223")),
            base_rootfs_path: expand_home(&env_or("SANDKILN_BASE_ROOTFS", "~/sandkiln-tools/images/ubuntu-22.04.ext4")),
            vcpu_count: env_or("SANDKILN_VCPU_COUNT", "2").parse().expect("SANDKILN_VCPU_COUNT must be a number"),
            mem_size_mib: env_or("SANDKILN_MEM_SIZE_MIB", "512").parse().expect("SANDKILN_MEM_SIZE_MIB must be a number"),
            bridge_name: env_or("SANDKILN_BRIDGE_NAME", "sktapbr0"),
            bridge_gateway: env_or("SANDKILN_BRIDGE_GATEWAY", "172.16.0.1")
                .parse()
                .expect("SANDKILN_BRIDGE_GATEWAY must be an IPv4 address"),
            uplink_iface: std::env::var("SANDKILN_UPLINK_IFACE").ok(),
            tap_pool_prefix: env_or("SANDKILN_TAP_POOL_PREFIX", "sktap"),
            tap_pool_size: env_or("SANDKILN_TAP_POOL_SIZE", "32").parse().expect("SANDKILN_TAP_POOL_SIZE must be a number"),
            auth_token: std::env::var("SANDKILN_AUTH_TOKEN").ok(),
            drives_dir: expand_home(&env_or("SANDKILN_DRIVES_DIR", "~/sandkiln-tools/drives")),
            log_format: LogFormat::from_env(),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
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
}

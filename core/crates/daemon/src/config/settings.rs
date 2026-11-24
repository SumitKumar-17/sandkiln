//! The registry of every daemon setting. One table drives everything that
//! used to be scattered across ad-hoc `env_or(...)` calls: value lookup,
//! unknown-key detection in the config file, `--print-config`, and the
//! generated reference table in the docs (`sandkilnd --config-reference`),
//! so the documented defaults cannot drift from the real ones.

pub struct Setting {
    /// Config-file key. The environment variable is `SANDKILN_` + the
    /// upper-cased key (see [`Setting::env_name`]).
    pub key: &'static str,
    /// `None` means "no default": unset is a meaningful state (for example
    /// no auth token, or a timeout that is disabled).
    pub default: Option<&'static str>,
    pub group: &'static str,
    pub doc: &'static str,
    /// Redacted by `--print-config`; a file holding one is checked for
    /// group/world-readable permissions.
    pub secret: bool,
}

impl Setting {
    pub fn env_name(&self) -> String {
        format!("SANDKILN_{}", self.key.to_ascii_uppercase())
    }
}

const fn s(key: &'static str, default: Option<&'static str>, group: &'static str, doc: &'static str) -> Setting {
    Setting { key, default, group, doc, secret: false }
}

pub const SETTINGS: &[Setting] = &[
    s("listen_addr", Some("127.0.0.1:7777"), "server", "Address the HTTP API listens on."),
    Setting {
        key: "auth_token",
        default: None,
        group: "server",
        doc: "Bearer token required on the REST API. Unset disables auth entirely: fine for localhost, never beyond.",
        secret: true,
    },
    s("log_format", Some("pretty"), "server", "`pretty` for terminals, `json` for one JSON object per log line."),
    s("preview_timeout_secs", Some("30"), "server", "How long a dev-server preview request waits before a 504."),
    s(
        "exec_timeout_secs",
        Some("600"),
        "server",
        "Longest a single `exec`, file or filesystem request may wait for the guest's reply. Also the default and the ceiling for an exec's own `timeout_secs`: on that limit the guest kills the command and the caller gets a 504 with the partial output. A reply that never comes at all is a plain 504 and the command may still be running.",
    ),
    s(
        "kernel_args",
        None,
        "vm",
        "Extra guest kernel command-line arguments, appended after sandkiln's own. Quieting kernel boot logging (the serial console is emulated) shortens time to first command.",
    ),
    s(
        "retained_checkpoints_max",
        Some("8"),
        "storage",
        "How many retired checkpoints (time-travel restore points, each a full-RAM snapshot plus a rootfs) to keep. Beyond this the oldest are deleted after each resume. 0 keeps all, which can fill the disk.",
    ),
    s(
        "default_warm_count",
        Some("2"),
        "pools",
        "Warm instances of the daemon's default shape (default image, vCPU and memory) kept ready in the background, so a plain `POST /sandboxes` resumes one (about 8ms to first command) instead of cold booting (about 210ms). Each is a held full-RAM snapshot on disk and holds one guest address; replenishment pauses when addresses or disk run low. 0 disables. Shown as the pool `default`.",
    ),
    s("firecracker_bin", Some("~/sandkiln-tools/bin/firecracker"), "vm", "Path to the Firecracker binary."),
    s("kernel_path", Some("~/sandkiln-tools/images/vmlinux-5.10.223"), "vm", "Guest kernel image (vmlinux)."),
    s("base_rootfs", Some("~/sandkiln-tools/images/ubuntu-22.04.ext4"), "vm", "Default base rootfs image sandboxes boot from."),
    s("vcpu_count", Some("2"), "vm", "Default vCPUs per sandbox."),
    s("mem_size_mib", Some("512"), "vm", "Default memory per sandbox, in MiB."),
    s("max_vcpu_count", Some("16"), "vm", "Ceiling on a per-sandbox vCPU override."),
    s("max_mem_size_mib", Some("16384"), "vm", "Ceiling on a per-sandbox memory override, in MiB."),
    s("state_dir", Some("~/sandkiln-tools/state"), "storage", "Durable state: held snapshots and retired checkpoints."),
    s(
        "rootfs_dir",
        None,
        "storage",
        "Per-sandbox writable rootfs copies. Defaults to `<state_dir>/rootfs`. Put it on the same reflink-capable filesystem (XFS, Btrfs) as `base_rootfs` for instant clones.",
    ),
    s("drives_dir", Some("~/sandkiln-tools/drives"), "storage", "Persistent drives."),
    s("images_dir", Some("~/sandkiln-tools/images-registered"), "storage", "Registered custom images."),
    s("history_db_path", Some("~/sandkiln-tools/history.db"), "storage", "SQLite database of durable sandbox lifecycle history."),
    s("archive_dir", Some("~/sandkiln-tools/archive"), "storage", "Where idle snapshots' state and memory files are archived to."),
    s("bridge_name", Some("sktapbr0"), "network", "Bridge all sandbox taps attach to."),
    s("bridge_gateway", Some("172.16.0.1"), "network", "Gateway IPv4 address on the bridge (a /24 is carved from it)."),
    s("uplink_iface", None, "network", "Host interface sandbox traffic NATs out of. Unset detects the default route."),
    s("tap_pool_prefix", Some("sktap"), "network", "Name prefix of the pre-created tap devices."),
    s("tap_pool_size", Some("32"), "network", "Number of pre-created tap devices. Must match `create-tap-pool.sh`; caps concurrent sandboxes."),
    s("idle_timeout_secs", None, "lifecycle", "Destroy a sandbox after this many idle seconds. Unset or 0 disables."),
    s("auto_suspend_timeout_secs", None, "lifecycle", "Suspend (snapshot) an idle sandbox instead. Must be strictly less than `idle_timeout_secs` when both are set. Unset or 0 disables."),
    s("archive_timeout_secs", None, "lifecycle", "Move an unresumed snapshot's state and memory files to `archive_dir` after this many seconds. Unset or 0 disables."),
    s("jailer_enabled", Some("false"), "jailer", "Boot sandboxes under the Firecracker jailer (chroot, cgroups, dedicated uid/gid). `true` or `1` enables."),
    s("jailer_bin", Some("~/sandkiln-tools/bin/jailer"), "jailer", "Path to the jailer binary (setuid root)."),
    s("jailer_chroot_base_dir", Some("~/sandkiln-tools/jail"), "jailer", "Base directory for per-VM chroots; keep on the same filesystem as rootfs and drives."),
    s("jailer_uid_gid_base", Some("600000"), "jailer", "First uid/gid of the range reserved for jailed VMs."),
    s("jailer_self_check", Some("true"), "jailer", "At startup, run the real jailer once against a throwaway jail and refuse to start if the daemon could not use the result. Disable only to diagnose a jailer setup."),
    s("jailer_pool_size", Some("32"), "jailer", "Size of the uid/gid range (max concurrent jailed VMs)."),
];

pub fn find(key: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.key == key)
}

/// Markdown reference for the docs, one table per group, generated from
/// [`SETTINGS`] so the documented defaults cannot drift from the real ones.
pub fn reference_markdown() -> String {
    let mut out = String::new();
    let mut groups: Vec<&str> = Vec::new();
    for s in SETTINGS {
        if !groups.contains(&s.group) {
            groups.push(s.group);
        }
    }
    for group in groups {
        out.push_str(&format!("### {group}\n\n| Key | Environment variable | Default | Description |\n|---|---|---|---|\n"));
        for s in SETTINGS.iter().filter(|s| s.group == group) {
            let default = s.default.map(|d| format!("`{d}`")).unwrap_or_else(|| "unset".to_string());
            out.push_str(&format!("| `{}` | `{}` | {} | {} |\n", s.key, s.env_name(), default, s.doc.replace('|', "\\|")));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn keys_are_unique_and_lowercase_snake_case() {
        let mut seen = HashSet::new();
        for s in SETTINGS {
            assert!(seen.insert(s.key), "duplicate key {}", s.key);
            assert!(s.key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'), "{}", s.key);
        }
    }

    #[test]
    fn env_names_keep_the_documented_sandkiln_prefix() {
        assert_eq!(find("base_rootfs").unwrap().env_name(), "SANDKILN_BASE_ROOTFS");
        assert_eq!(find("idle_timeout_secs").unwrap().env_name(), "SANDKILN_IDLE_TIMEOUT_SECS");
    }

    #[test]
    fn reference_lists_every_setting() {
        let md = reference_markdown();
        for s in SETTINGS {
            assert!(md.contains(&format!("`{}`", s.env_name())), "missing {}", s.key);
        }
    }
}

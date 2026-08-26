use std::path::PathBuf;

pub struct Config {
    pub listen_addr: String,
    pub firecracker_bin: PathBuf,
    pub kernel_path: PathBuf,
    pub base_rootfs_path: PathBuf,
    pub vcpu_count: u8,
    pub mem_size_mib: u32,
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

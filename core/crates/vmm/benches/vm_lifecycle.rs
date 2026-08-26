//! Criterion benchmarks against a real Firecracker binary — no mocking.
//! Needs KVM and real kernel/rootfs/firecracker assets on the machine
//! running it, pointed to via env vars (see `required_path` below).
//!
//! Usage:
//!   SANDKILN_BENCH_FIRECRACKER_BIN=~/sandkiln-tools/bin/firecracker \
//!   SANDKILN_BENCH_KERNEL_PATH=~/sandkiln-tools/images/vmlinux-5.10.223 \
//!   SANDKILN_BENCH_ROOTFS_PATH=~/sandkiln-tools/images/ubuntu-22.04.ext4 \
//!   cargo bench -p sandkiln-vmm --bench vm_lifecycle

use criterion::{criterion_group, criterion_main, Criterion};
use sandkiln_protocol::Request;
use sandkiln_vmm::vm::{Vm, VmConfig};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

struct BenchConfig {
    firecracker_bin: PathBuf,
    kernel_path: PathBuf,
    base_rootfs_path: PathBuf,
    vcpu_count: u8,
    mem_size_mib: u32,
}

impl BenchConfig {
    fn from_env() -> Self {
        Self {
            firecracker_bin: required_path("SANDKILN_BENCH_FIRECRACKER_BIN"),
            kernel_path: required_path("SANDKILN_BENCH_KERNEL_PATH"),
            base_rootfs_path: required_path("SANDKILN_BENCH_ROOTFS_PATH"),
            vcpu_count: env_or("SANDKILN_BENCH_VCPU_COUNT", "2")
                .parse()
                .expect("SANDKILN_BENCH_VCPU_COUNT must be a number"),
            mem_size_mib: env_or("SANDKILN_BENCH_MEM_SIZE_MIB", "512")
                .parse()
                .expect("SANDKILN_BENCH_MEM_SIZE_MIB must be a number"),
        }
    }

    fn vm_config(&self, rootfs_path: PathBuf) -> VmConfig {
        VmConfig {
            firecracker_bin: self.firecracker_bin.clone(),
            kernel_path: self.kernel_path.clone(),
            rootfs_path,
            vcpu_count: self.vcpu_count,
            mem_size_mib: self.mem_size_mib,
            network: None,
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Unlike the daemon's config (which has real defaults for a dev box),
/// these benches can't guess at asset paths — an unset env var means "not
/// configured for this machine," not "assume ~/sandkiln-tools". Fail loudly
/// with the exact command to set up instead of silently skipping.
fn required_path(key: &str) -> PathBuf {
    let raw = std::env::var(key).unwrap_or_else(|_| {
        panic!(
            "\n\n{key} is not set.\n\n\
             This benchmark boots real Firecracker VMs and needs real assets\n\
             on this machine. Set:\n  \
             SANDKILN_BENCH_FIRECRACKER_BIN  - path to the firecracker binary\n  \
             SANDKILN_BENCH_KERNEL_PATH      - path to a vmlinux kernel image\n  \
             SANDKILN_BENCH_ROOTFS_PATH      - path to a base rootfs image (copied per iteration)\n\n\
             e.g.:\n  \
             SANDKILN_BENCH_FIRECRACKER_BIN=~/sandkiln-tools/bin/firecracker \\\n  \
             SANDKILN_BENCH_KERNEL_PATH=~/sandkiln-tools/images/vmlinux-5.10.223 \\\n  \
             SANDKILN_BENCH_ROOTFS_PATH=~/sandkiln-tools/images/ubuntu-22.04.ext4 \\\n  \
             cargo bench -p sandkiln-vmm --bench vm_lifecycle\n"
        )
    });
    let expanded = expand_home(&raw);
    if !expanded.exists() {
        panic!("{key}={raw:?} does not exist on disk (resolved to {expanded:?})");
    }
    expanded
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").expect("HOME must be set to expand ~")).join(rest),
        None => PathBuf::from(path),
    }
}

static ROOTFS_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `Vm::boot` writes to `rootfs_path` in place, so every booted VM (in the
/// daemon and here) needs its own disposable copy of the base image.
fn fresh_rootfs_copy(base: &Path) -> PathBuf {
    let n = ROOTFS_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dest = std::env::temp_dir().join(format!("sandkiln-bench-rootfs-{}-{n}.ext4", std::process::id()));
    std::fs::copy(base, &dest).expect("failed to copy base rootfs for benchmark iteration");
    dest
}

/// Cold boot: `Vm::boot()` to a returned handle, fresh VM and fresh rootfs
/// copy per iteration. Rootfs copy and `vm.stop()` happen outside the
/// timed region — only the boot itself is measured.
fn bench_cold_boot(c: &mut Criterion) {
    let config = BenchConfig::from_env();

    let mut group = c.benchmark_group("vm_lifecycle");
    // A cold boot spawns a real Firecracker process every iteration; keep
    // sample count and warm-up modest so the bench finishes in a
    // reasonable time instead of booting hundreds of VMs.
    group.sample_size(20);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(20));

    group.bench_function("cold_boot", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                let rootfs_path = fresh_rootfs_copy(&config.base_rootfs_path);
                let vm_config = config.vm_config(rootfs_path.clone());

                let started = Instant::now();
                let vm = Vm::boot(&vm_config).expect("Vm::boot failed during benchmark");
                total += started.elapsed();

                vm.stop().expect("Vm::stop failed during benchmark cleanup");
                let _ = std::fs::remove_file(&rootfs_path);
            }
            total
        });
    });

    group.finish();
}

/// Exec round-trip latency: one VM booted once outside the timed loop,
/// then repeated `vm.call(&Request::Exec { command: "true", .. })` over
/// the existing vsock connection.
fn bench_exec_roundtrip(c: &mut Criterion) {
    let config = BenchConfig::from_env();

    let rootfs_path = fresh_rootfs_copy(&config.base_rootfs_path);
    let vm = Vm::boot(&config.vm_config(rootfs_path.clone())).expect("Vm::boot failed setting up exec benchmark");
    let request = Request::Exec { command: "true".to_string(), args: vec![] };

    let mut group = c.benchmark_group("vm_lifecycle");
    group.bench_function("exec_roundtrip", |b| {
        b.iter(|| vm.call(&request).expect("vm.call failed during benchmark"));
    });
    group.finish();

    vm.stop().expect("Vm::stop failed tearing down exec benchmark");
    let _ = std::fs::remove_file(&rootfs_path);
}

criterion_group!(benches, bench_cold_boot, bench_exec_roundtrip);
criterion_main!(benches);

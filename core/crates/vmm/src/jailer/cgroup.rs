//! cgroup v2 for jailed VMs: the limits passed to jailer, removal of a
//! finished VM's cgroup, and the startup sweep of jails a previous daemon
//! process left behind.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Where jailer's cgroup v2 hierarchy lives: `<root>/<exec file name>/<id>`.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Where jailer puts one VM's cgroup. Jailer never removes it, so every
/// jailed boot would otherwise leak one for the life of the host.
pub fn cgroup_dir(supervisor_bin: &Path, jail_instance_id: &str) -> PathBuf {
    let exec_name = supervisor_bin.file_name().expect("supervisor_bin must have a file name");
    Path::new(CGROUP_ROOT).join(exec_name).join(jail_instance_id)
}

/// Removes a finished VM's cgroup. Works for the unprivileged daemon only
/// when it owns the parent cgroup directory (`prepare-jailer.sh` arranges
/// that): removing a cgroup needs write access to its parent, not to the
/// cgroup itself. An already-missing cgroup is success.
pub fn remove_cgroup(supervisor_bin: &Path, jail_instance_id: &str) -> io::Result<()> {
    match fs::remove_dir(cgroup_dir(supervisor_bin, jail_instance_id)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// What one startup sweep found.
#[derive(Debug, Default, PartialEq)]
pub struct SweepReport {
    pub removed: Vec<String>,
    /// Jails that could not be removed, with the reason.
    pub failed: Vec<(String, String)>,
}

/// Removes jails (and their cgroups) a previous daemon process left behind.
/// A jailed VM cannot outlive its daemon (the supervisor ends Firecracker
/// when the daemon's pipe closes), so at startup every `sandkiln-*` jail is
/// stale. It also matters for correctness: jail ids restart from 1 with each
/// process, so a leftover `sandkiln-1` would collide with the next boot.
pub fn sweep_stale_jails(chroot_base_dir: &Path, supervisor_bin: &Path) -> SweepReport {
    sweep_stale_jails_in(chroot_base_dir, supervisor_bin, Path::new(CGROUP_ROOT))
}

fn sweep_stale_jails_in(chroot_base_dir: &Path, supervisor_bin: &Path, cgroup_root: &Path) -> SweepReport {
    let mut report = SweepReport::default();
    let exec_name = supervisor_bin.file_name().expect("supervisor_bin must have a file name");
    let jails = chroot_base_dir.join(exec_name);
    let Ok(entries) = fs::read_dir(&jails) else { return report };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("sandkiln-") {
            continue;
        }
        match fs::remove_dir_all(entry.path()) {
            Ok(()) => report.removed.push(name.clone()),
            Err(e) => report.failed.push((name.clone(), e.to_string())),
        }
        // A missing cgroup is the normal case; a busy one means a process
        // still runs in it, which is reported by the failed jail removal.
        let _ = fs::remove_dir(cgroup_root.join(exec_name).join(&name));
    }
    report
}

/// cgroup v2 memory ceiling: guest RAM plus headroom for Firecracker's
/// own VMM overhead (page tables, virtio buffers, vsock/balloon
/// backends). Without this margin a VM sized right at its cgroup ceiling
/// gets OOM-killed before the guest finishes booting — the ceiling bounds
/// the whole jailed process, not just guest-visible RAM.
pub fn cgroup_memory_max_bytes(mem_size_mib: u32) -> u64 {
    const VMM_OVERHEAD_MIB: u64 = 128;
    (mem_size_mib as u64 + VMM_OVERHEAD_MIB) * 1024 * 1024
}

/// cgroup v2's `cpu.max` (`"<quota> <period>"` in microseconds), pinning
/// one jailed VM to at most `vcpu_count` fully-utilized cores — without it
/// a runaway guest can starve other sandboxes' vCPU threads, which are
/// ordinary host threads with no isolation of their own beyond this.
pub fn cgroup_cpu_max(vcpu_count: u8) -> String {
    const PERIOD_US: u64 = 100_000;
    format!("{} {PERIOD_US}", vcpu_count as u64 * PERIOD_US)
}

/// The `--cgroup <controller>.<key>=<value>` values to pass to jailer for
/// one VM's resource ceiling, derived from the same `vcpu_count`/
/// `mem_size_mib` already used for Firecracker's own `/machine-config`.
pub fn cgroup_limits(mem_size_mib: u32, vcpu_count: u8) -> Vec<String> {
    vec![format!("memory.max={}", cgroup_memory_max_bytes(mem_size_mib)), format!("cpu.max={}", cgroup_cpu_max(vcpu_count))]
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TempDir;
    use super::*;

    #[test]
    fn cgroup_memory_max_bytes_adds_vmm_overhead_margin() {
        let bytes = cgroup_memory_max_bytes(512);
        assert_eq!(bytes, (512 + 128) * 1024 * 1024);
        assert!(bytes > 512 * 1024 * 1024, "ceiling must exceed the guest's own configured RAM");
    }

    #[test]
    fn cgroup_cpu_max_scales_quota_linearly_with_vcpu_count() {
        assert_eq!(cgroup_cpu_max(1), "100000 100000");
        assert_eq!(cgroup_cpu_max(2), "200000 100000");
        assert_eq!(cgroup_cpu_max(4), "400000 100000");
    }

    #[test]
    fn cgroup_limits_includes_both_memory_and_cpu_controllers() {
        let limits = cgroup_limits(512, 2);
        assert_eq!(limits.len(), 2);
        assert!(limits[0].starts_with("memory.max="));
        assert!(limits[1].starts_with("cpu.max="));
    }


    #[test]
    fn cgroup_dir_follows_the_exec_file_name_then_the_instance_id() {
        assert_eq!(
            cgroup_dir(Path::new("/opt/sandkiln-firecracker-supervisor"), "sandkiln-3"),
            PathBuf::from("/sys/fs/cgroup/sandkiln-firecracker-supervisor/sandkiln-3")
        );
    }

    #[test]
    fn sweep_removes_only_sandkiln_jails_and_their_cgroups() {
        let tmp = TempDir::new("sweep");
        let supervisor = Path::new("/opt/sandkiln-firecracker-supervisor");
        let jails = tmp.path.join("jail").join("sandkiln-firecracker-supervisor");
        fs::create_dir_all(jails.join("sandkiln-1").join("root")).unwrap();
        fs::create_dir_all(jails.join("sandkiln-2")).unwrap();
        fs::create_dir_all(jails.join("someone-elses")).unwrap();
        let cgroups = tmp.path.join("cg");
        fs::create_dir_all(cgroups.join("sandkiln-firecracker-supervisor").join("sandkiln-1")).unwrap();

        let mut report = sweep_stale_jails_in(&tmp.path.join("jail"), supervisor, &cgroups);
        report.removed.sort();

        assert_eq!(report.removed, vec!["sandkiln-1".to_string(), "sandkiln-2".to_string()]);
        assert!(report.failed.is_empty());
        assert!(jails.join("someone-elses").exists(), "a directory that is not ours must be left alone");
        assert!(!jails.join("sandkiln-1").exists());
        assert!(!cgroups.join("sandkiln-firecracker-supervisor").join("sandkiln-1").exists());
    }

    #[test]
    fn sweep_with_no_jail_directory_is_a_no_op() {
        let tmp = TempDir::new("sweep-empty");
        let report = sweep_stale_jails_in(&tmp.path, Path::new("/opt/sandkiln-firecracker-supervisor"), &tmp.path);
        assert_eq!(report, SweepReport::default());
    }

    #[test]
    fn removing_a_cgroup_that_is_already_gone_is_success() {
        assert!(remove_cgroup(Path::new("/opt/no-such-supervisor-for-test"), "sandkiln-1").is_ok());
    }

}

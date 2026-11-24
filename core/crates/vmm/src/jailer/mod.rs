//! Firecracker's jailer: re-execs `firecracker` inside a chroot'd,
//! cgroup-limited environment under a dedicated unprivileged uid/gid,
//! instead of `vm::boot`'s default direct spawn. See
//! `docs/design-notes/security-hardening.md` and the jailer setup in
//! `docs-site/self-hosting/manual-setup.mdx`.
//!
//! **Why the daemon can't do this itself**: chroot(2), setuid/setgid,
//! `/dev/kvm`/`/dev/net/tun` device nodes, and cgroup management all need
//! privileges the daemon deliberately doesn't have (ambient
//! `CAP_NET_ADMIN` only — see root `AGENTS.md`). Jailer holds those
//! privileges only for the brief setup window, then drops them and execs
//! `firecracker` as the target uid/gid — the standard fix is making
//! `jailer` itself setuid-root, a small purpose-built binary, not the
//! whole daemon.
//!
//! **Who owns what, and how the daemon reaches the jail.** Jailer runs the
//! `--exec-file` as the per-VM uid (distinct per concurrent VM) and the shared
//! jail group (`JailLaunch::gid`), which the daemon's user belongs to. The
//! exec file is not Firecracker but `sandkiln-firecracker-supervisor`
//! (`crates/jail-supervisor`): jailer makes the chroot root mode 0700, and
//! the supervisor, which owns it, widens it to the jail group before running
//! Firecracker, so the daemon can reach the API and vsock sockets and remove
//! the jail afterwards. The supervisor also ends Firecracker when the pipe
//! the daemon holds to its stdin closes, because the daemon may not signal a
//! process of another uid. See `docs/design-notes/security-hardening.md` and
//! finding 32 in `docs/findings.md` for the options weighed.
//!
//! **What crosses the chroot boundary**: once jailer calls `chroot()`,
//! firecracker can't see any host path outside its jail root. Every
//! referenced file (kernel, rootfs, drives) must exist inside the jail
//! *before* jailer starts, and every path in Firecracker's own API
//! afterward must be the in-jail path. `link_resource_into_jail` puts a
//! host file in the jail; `vm::boot_inner` rewrites API bodies to the
//! resulting in-jail paths.
//!
//! The module is split by what each part owns: `ids` (uid pool, instance
//! ids), `chroot` (the jail directory tree and what is placed in it),
//! `cgroup` (limits, removal, the startup sweep) and `selfcheck` (the startup
//! probe). This file holds the launch parameters and the jailer argv, and
//! re-exports every public item so callers use `jailer::X` throughout.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

mod cgroup;
mod chroot;
mod ids;
mod selfcheck;
#[cfg(test)]
mod test_support;

pub use cgroup::{cgroup_cpu_max, cgroup_dir, cgroup_limits, cgroup_memory_max_bytes, remove_cgroup, sweep_stale_jails, SweepReport};
pub use chroot::{chroot_root, copy_binary_into_jail, instance_dir, link_resource_into_jail, prepare_chroot_dir, JailedPath};
pub use ids::{jail_instance_id, JailerIdPool};
pub use selfcheck::{is_setuid_root, self_check};

/// One VM's fully-resolved jail launch parameters — everything
/// `vm::boot_inner`/`vm::resume` need to invoke jailer for one microVM.
/// `uid` must come from a [`JailerIdPool`] (or an equivalent allocation the
/// caller guarantees is unique among concurrently running jailed VMs);
/// nothing here re-validates that.
#[derive(Clone)]
pub struct JailLaunch {
    pub jailer_bin: PathBuf,
    /// Runs in the chroot in place of `firecracker`; see the module docs.
    /// Its file name must contain "firecracker" (jailer insists).
    pub supervisor_bin: PathBuf,
    pub chroot_base_dir: PathBuf,
    /// Per-VM: a distinct uid for every concurrently running jailed VM.
    pub uid: u32,
    /// Shared by every jailed VM, and the daemon's user must be a member.
    /// It is what gives the daemon access to the jail and to the tap pool
    /// (`scripts/host-setup/prepare-jailer.sh`).
    pub gid: u32,
}

/// Builds the full jailer argv (excluding argv[0]) for one VM. Pure and
/// testable — `vm::boot_inner`/`vm::resume` pass this straight to
/// `Command::args`.
///
/// Omits `--daemonize`: jailer stays attached to its parent and `exec`s
/// firecracker in place rather than forking/detaching, so the `Child`
/// handle `Command::spawn` returns *becomes* the firecracker process
/// after exec (same pid) — `Vm::stop`'s existing `kill()`/`wait()`
/// lifecycle keeps working unchanged.
///
/// Omits `--netns`: tap devices attach from a shared pool directly in the
/// daemon's own network namespace (`network.rs`), no per-VM namespace to
/// join. Network-namespace isolation is a separate, not-yet-built axis
/// from chroot/cgroup/uid isolation.
pub fn build_jailer_args(
    launch: &JailLaunch,
    jail_instance_id: &str,
    cgroup_limits: &[String],
    api_sock_in_jail: &Path,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        OsString::from("--id"),
        OsString::from(jail_instance_id),
        OsString::from("--exec-file"),
        launch.supervisor_bin.as_os_str().to_owned(),
        OsString::from("--uid"),
        OsString::from(launch.uid.to_string()),
        OsString::from("--gid"),
        OsString::from(launch.gid.to_string()),
        OsString::from("--chroot-base-dir"),
        launch.chroot_base_dir.as_os_str().to_owned(),
        OsString::from("--cgroup-version"),
        OsString::from("2"),
    ];
    for limit in cgroup_limits {
        args.push(OsString::from("--cgroup"));
        args.push(OsString::from(limit));
    }
    args.push(OsString::from("--"));
    args.push(OsString::from("--api-sock"));
    args.push(api_sock_in_jail.as_os_str().to_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(uid: u32, gid: u32) -> JailLaunch {
        JailLaunch {
            jailer_bin: PathBuf::from("/usr/bin/jailer"),
            supervisor_bin: PathBuf::from("/opt/sandkiln-firecracker-supervisor"),
            chroot_base_dir: PathBuf::from("/srv/jailer"),
            uid,
            gid,
        }
    }

    #[test]
    fn build_jailer_args_places_the_separator_before_the_supervisors_own_args() {
        let args = build_jailer_args(&launch(600001, 599000), "sandkiln-9", &["memory.max=1073741824".to_string()], Path::new("/api.sock"));

        let separator_index = args.iter().position(|a| a == "--").expect("must contain a -- separator");
        let after: Vec<&OsString> = args[separator_index + 1..].iter().collect();
        assert_eq!(after, vec![&OsString::from("--api-sock"), &OsString::from("/api.sock")]);

        let before: Vec<String> = args[..separator_index].iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(before.contains(&"sandkiln-9".to_string()));
        assert!(before.contains(&"memory.max=1073741824".to_string()));
        assert!(before.contains(&"2".to_string()), "cgroup-version 2 must always be requested");
    }

    #[test]
    fn build_jailer_args_runs_the_supervisor_under_the_vms_uid_and_the_shared_gid() {
        let args: Vec<String> = build_jailer_args(&launch(600001, 599000), "x", &[], Path::new("/api.sock"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let value_after = |flag: &str| args[args.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!(value_after("--exec-file"), "/opt/sandkiln-firecracker-supervisor");
        assert_eq!(value_after("--uid"), "600001");
        assert_eq!(value_after("--gid"), "599000");
    }

    #[test]
    fn build_jailer_args_omits_daemonize_and_netns() {
        let args = build_jailer_args(&launch(1, 1), "sandkiln-1", &[], Path::new("/api.sock"));
        let joined: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(!joined.contains(&"--daemonize".to_string()));
        assert!(!joined.contains(&"--netns".to_string()));
    }
}

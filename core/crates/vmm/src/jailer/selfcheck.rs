//! The startup probe for `jailer_enabled`: runs the real jailer once against
//! a throwaway jail and checks the daemon can use what it produced.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

use super::cgroup::{cgroup_dir, cgroup_limits, remove_cgroup};
use super::chroot::{chroot_root, instance_dir, prepare_chroot_dir};
use super::{build_jailer_args, JailLaunch};

/// Jail instance id of the startup self-check's throwaway jail.
const SELF_CHECK_INSTANCE: &str = "sandkiln-selfcheck";

/// How long the self-check waits for its throwaway jailer run to finish.
const SELF_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether `meta` describes a setuid-root file: the way the unprivileged
/// daemon is allowed to run `jailer` (chroot, setuid, cgroups, device nodes
/// all need root).
pub fn is_setuid_root(meta: &fs::Metadata) -> bool {
    meta.mode() & 0o4000 != 0 && meta.uid() == 0
}

/// Turns the result of "can the daemon use the jail root" into the startup
/// verdict. Pure so the wording and the classification are tested without a
/// real jailer. A permission error means the supervisor's widening did not
/// reach the daemon: the finding 32 failure (`docs/findings.md`).
fn judge_jail_access(access: io::Result<()>, root: &Path, gid: u32) -> Result<(), String> {
    match access {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(format!(
            "the daemon cannot use the jail root {} (finding 32 in docs/findings.md). Jailer makes it mode 0700 and the jail supervisor \
             opens it to group {gid}, so the daemon's user must be a member of group {gid} in the session that started it \
             (check `id`; a group added after login is not picked up). Run scripts/host-setup/prepare-jailer.sh, see \
             docs-site/self-hosting/jailer.mdx. Underlying error: {e}",
            root.display()
        )),
        Err(e) => Err(format!("cannot use the jail root {}: {e}", root.display())),
    }
}

/// What the daemon must be able to do in a jail after the supervisor ran:
/// enter the root, create and remove an entry in it (teardown deletes the
/// whole tree the same way).
fn exercise_jail(root: &Path) -> io::Result<()> {
    fs::read_dir(root)?;
    let probe = root.join(".sandkiln-probe");
    fs::write(&probe, b"")?;
    fs::remove_file(&probe)
}

/// Startup probe for `jailer_enabled`: runs the real `jailer` once against a
/// throwaway jail whose supervisor exits immediately (`--self-check`), then
/// checks, as the daemon's own user, that it can use the jail root, tear the
/// jail down and remove the cgroup. Those are exactly the accesses a real
/// boot depends on. One run also proves the other prerequisites: `jailer` is
/// setuid root, cgroup v2 is usable, the uid is accepted, the chroot base is
/// writable, the supervisor is a static binary that runs in an empty chroot.
///
/// Why a probe and not a static check: what the daemon can do in the jail
/// depends on what jailer does to ownership and modes and on the daemon's
/// group membership, which is only observable by running it. A failed probe
/// can leave root-owned files only root can remove (one fixed jail name,
/// reused by the next probe); the error names it.
pub fn self_check(launch: &JailLaunch) -> Result<(), String> {
    let meta = fs::metadata(&launch.jailer_bin).map_err(|e| format!("cannot stat jailer binary {}: {e}", launch.jailer_bin.display()))?;
    if !is_setuid_root(&meta) {
        return Err(format!(
            "{} is not setuid root; jailer needs root for chroot, setuid and cgroups. Run: sudo chown root:root {0} && sudo chmod u+s {0}",
            launch.jailer_bin.display()
        ));
    }
    if !launch.supervisor_bin.is_file() {
        return Err(format!(
            "the jail supervisor {} does not exist; build it with `cargo build --release -p sandkiln-jail-supervisor --target x86_64-unknown-linux-musl` and install it (scripts/host-setup/prepare-jailer.sh)",
            launch.supervisor_bin.display()
        ));
    }
    run_probe(launch)
}

/// The probe itself, past the static gates: one jailer run, then the access
/// tests. Separate from [`self_check`] so tests can drive it with a stand-in
/// jailer that is not setuid root.
fn run_probe(launch: &JailLaunch) -> Result<(), String> {
    use std::process::{Command, Stdio};

    let root = chroot_root(&launch.chroot_base_dir, &launch.supervisor_bin, SELF_CHECK_INSTANCE);
    let instance = instance_dir(&root);
    let _ = fs::remove_dir_all(&instance);
    let _ = remove_cgroup(&launch.supervisor_bin, SELF_CHECK_INSTANCE);
    prepare_chroot_dir(&root).map_err(|e| format!("cannot create the probe jail under {}: {e}", launch.chroot_base_dir.display()))?;

    // Same argv as a real boot (cgroup limits included: jailer cannot move itself
    // into the shared parent cgroup without them) up to the separator, then the
    // supervisor's own no-op mode.
    let mut args = build_jailer_args(launch, SELF_CHECK_INSTANCE, &cgroup_limits(128, 1), Path::new("/api.sock"));
    let separator = args.iter().position(|a| a == "--").expect("build_jailer_args always emits `--`");
    args.truncate(separator + 1);
    args.push(OsString::from("--self-check"));
    let mut child = Command::new(&launch.jailer_bin)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run {}: {e}", launch.jailer_bin.display()))?;

    let deadline = Instant::now() + SELF_CHECK_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                return Err(format!("the jailer probe did not finish within {SELF_CHECK_TIMEOUT:?}"));
            }
            Err(e) => return Err(format!("waiting for the jailer probe failed: {e}")),
        }
    };
    if !status.success() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = io::Read::read_to_string(&mut pipe, &mut stderr);
        }
        return Err(format!("the jailer probe failed ({status}): {}", stderr.trim()));
    }

    let verdict = judge_jail_access(exercise_jail(&root), &root, launch.gid);
    if verdict.is_err() {
        return verdict.map_err(|e| match fs::remove_dir_all(&instance) {
            Ok(()) => e,
            Err(_) => format!("{e}. The probe left {} behind; remove it with sudo", instance.display()),
        });
    }
    fs::remove_dir_all(&instance).map_err(|e| format!("cannot remove the probe jail {}: {e}", instance.display()))?;
    remove_cgroup(&launch.supervisor_bin, SELF_CHECK_INSTANCE).map_err(|e| {
        format!(
            "cannot remove the probe's cgroup {}: {e}. The daemon must own the parent cgroup directory so it can remove each VM's cgroup (scripts/host-setup/prepare-jailer.sh)",
            cgroup_dir(&launch.supervisor_bin, SELF_CHECK_INSTANCE).display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::TempDir;
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    fn setuid_root_needs_both_the_bit_and_root_ownership() {
        let tmp = TempDir::new("setuid-root");
        let file = tmp.path.join("jailer");
        fs::write(&file, b"x").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o4755)).unwrap();
        // Owned by the test user, not root: the bit alone is not enough.
        let meta = fs::metadata(&file).unwrap();
        assert_eq!(is_setuid_root(&meta), meta.uid() == 0);
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!is_setuid_root(&fs::metadata(&file).unwrap()));
    }

    #[test]
    fn judge_jail_access_names_finding_32_on_permission_denied() {
        let verdict = judge_jail_access(Err(io::Error::from(io::ErrorKind::PermissionDenied)), Path::new("/j/root"), 600001);
        let msg = verdict.unwrap_err();
        assert!(msg.contains("finding 32") && msg.contains("/j/root") && msg.contains("group 600001"), "{msg}");
    }

    #[test]
    fn judge_jail_access_passes_through_other_errors_and_success() {
        assert!(judge_jail_access(Ok(()), Path::new("/j/root"), 1).is_ok());
        let msg = judge_jail_access(Err(io::Error::from(io::ErrorKind::NotFound)), Path::new("/j/root"), 1).unwrap_err();
        assert!(!msg.contains("finding 32"), "{msg}");
    }

    /// A stand-in `jailer` shell script: parses `--id`/`--chroot-base-dir`/
    /// `--exec-file` like the real one, creates the jail root, then applies
    /// `root_mode` to it. Not setuid root, so `self_check`'s first gate is
    /// bypassed by calling `run_probe` directly.
    fn fake_jailer(dir: &Path, root_mode: &str, exit_code: i32) -> PathBuf {
        let script = dir.join("fake-jailer");
        let body = format!(
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do case $1 in --id) id=$2; shift;; --chroot-base-dir) base=$2; shift;; --exec-file) exe=$2; shift;; esac; shift; done\n\
             root=\"$base/$(basename $exe)/$id/root\"; mkdir -p \"$root\"; chmod {root_mode} \"$root\"; echo probe-stderr >&2; exit {exit_code}\n"
        );
        fs::write(&script, body).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn probe_launch(tmp: &TempDir, jailer: PathBuf) -> JailLaunch {
        JailLaunch {
            jailer_bin: jailer,
            supervisor_bin: PathBuf::from("/usr/bin/sandkiln-firecracker-supervisor"),
            chroot_base_dir: tmp.path.join("jail"),
            uid: 600000,
            gid: 600000,
        }
    }

    #[test]
    fn self_check_rejects_a_jailer_that_is_not_setuid_root() {
        let tmp = TempDir::new("selfcheck-not-setuid");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "700", 0));
        let err = self_check(&launch).unwrap_err();
        assert!(err.contains("not setuid root"), "{err}");
    }

    #[test]
    fn self_check_reports_a_missing_jailer_binary() {
        let tmp = TempDir::new("selfcheck-missing");
        let launch = probe_launch(&tmp, tmp.path.join("no-such-jailer"));
        assert!(self_check(&launch).unwrap_err().contains("cannot stat"));
    }

    #[test]
    fn probe_reports_the_jailers_own_failure_output() {
        let tmp = TempDir::new("probe-jailer-fails");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "755", 3));
        let msg = run_probe(&launch).unwrap_err();
        assert!(msg.contains("probe failed") && msg.contains("probe-stderr"), "{msg}");
    }

    #[test]
    fn probe_passes_when_the_daemon_can_enter_the_jail_root() {
        let tmp = TempDir::new("probe-ok");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "755", 0));
        assert!(run_probe(&launch).is_ok());
    }

    #[test]
    fn probe_detects_a_jail_root_the_daemon_cannot_enter() {
        // As root every directory is enterable, so the condition cannot be built.
        if std::process::Command::new("id").arg("-u").output().map(|o| o.stdout == b"0\n").unwrap_or(false) {
            return;
        }
        let tmp = TempDir::new("probe-bad");
        let launch = probe_launch(&tmp, fake_jailer(&tmp.path, "000", 0));
        let msg = run_probe(&launch).unwrap_err();
        assert!(msg.contains("finding 32"), "{msg}");
        // Let TempDir's drop remove what the stand-in locked.
        let root = chroot_root(&launch.chroot_base_dir, &launch.supervisor_bin, SELF_CHECK_INSTANCE);
        fs::set_permissions(root, fs::Permissions::from_mode(0o755)).unwrap();
    }

}

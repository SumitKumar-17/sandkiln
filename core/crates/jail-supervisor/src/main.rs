//! The process jailer runs inside a sandbox's chroot in place of
//! `firecracker` (`--exec-file`), as that sandbox's own unprivileged uid and
//! the shared jail group. It exists to close two gaps that the stock jailer
//! leaves for an unprivileged daemon (finding 32 in `docs/findings.md`):
//!
//! * Reach. Jailer makes the chroot root mode 0700, so the daemon cannot
//!   enter it to reach Firecracker's API and vsock sockets. This process owns
//!   that directory (jailer just chowned it), so it widens it to the jail
//!   group, which the daemon belongs to, and sets umask 007 so the sockets
//!   Firecracker creates are group-accessible too. No privilege is involved:
//!   an owner changing the mode of its own directory.
//! * Stop. The daemon cannot signal a process of another uid. The daemon
//!   keeps the write end of this process's stdin; closing it (an explicit
//!   stop, or the daemon dying) makes this process SIGKILL Firecracker and
//!   exit, so a jailed VM can never outlive its daemon.
//!
//! Must be a static binary: it runs in an otherwise empty chroot.

use std::ffi::OsString;
use std::io::{self, Read};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::thread;

/// Where the daemon links the real Firecracker binary, in the jail.
const FIRECRACKER: &str = "/firecracker";

/// Group read/write/search plus setgid, so files the daemon creates in the
/// jail (the tunnel listener socket) belong to the jail group and
/// Firecracker can use them.
const GROUP_DIR_MODE: u32 = 0o2770;

/// The directories jailer creates in the chroot (its `FOLDER_HIERARCHY`),
/// relative to the chroot root. The daemon removes them on teardown, which
/// needs write access to each.
const JAIL_DIRS: [&str; 4] = ["", "dev", "dev/net", "run"];

#[derive(Debug, PartialEq)]
enum Mode {
    /// Widen the directories and exit; the daemon's startup probe.
    SelfCheck,
    /// Widen, then run Firecracker with these arguments.
    Run(Vec<OsString>),
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Mode {
    let args: Vec<OsString> = args.into_iter().collect();
    if args.iter().any(|a| a == "--self-check") {
        Mode::SelfCheck
    } else {
        Mode::Run(args)
    }
}

/// Makes the jail directories group-accessible. A directory jailer did not
/// create (e.g. `run` on some versions) is skipped; anything else failing
/// is an error, since the daemon could then not reach the jail.
fn open_dirs_to_group(chroot_root: &Path) -> io::Result<()> {
    for rel in JAIL_DIRS {
        match std::fs::set_permissions(chroot_root.join(rel), std::fs::Permissions::from_mode(GROUP_DIR_MODE)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound && !rel.is_empty() => {}
            Err(e) => return Err(io::Error::new(e.kind(), format!("chmod {}: {e}", chroot_root.join(rel).display()))),
        }
    }
    Ok(())
}

/// Runs `program` and SIGKILLs it when `control` reaches end of file (or
/// errors). Returns the program's exit status.
fn supervise(program: &Path, args: &[OsString], mut control: impl Read + Send + 'static) -> io::Result<ExitStatus> {
    // Not `Stdio::null()`: that opens /dev/null, which does not exist in the
    // jail. A pipe whose write end is dropped at once reads as end of file.
    let mut child = Command::new(program).args(args).stdin(Stdio::piped()).spawn()?;
    drop(child.stdin.take());
    let pid = child.id() as libc::pid_t;
    thread::spawn(move || {
        let mut buf = [0u8; 64];
        // Any bytes are ignored; only the end of the stream means "stop".
        while matches!(control.read(&mut buf), Ok(n) if n > 0) {}
        // SAFETY: plain kill(2) on a pid this process spawned and still owns
        // (it is only reaped after this thread's process exits or the child
        // has already been waited on, in which case kill fails harmlessly).
        unsafe { libc::kill(pid, libc::SIGKILL) };
    });
    child.wait()
}

fn exit_code(status: ExitStatus) -> u8 {
    status.code().map(|c| c as u8).unwrap_or_else(|| 128u8.wrapping_add(status.signal().unwrap_or(0) as u8))
}

fn main() -> ExitCode {
    // SAFETY: umask(2) cannot fail and only affects this process.
    unsafe { libc::umask(0o007) };
    let mode = parse_args(std::env::args_os().skip(1));

    if let Err(e) = open_dirs_to_group(Path::new("/")) {
        eprintln!("sandkiln-firecracker-supervisor: {e}");
        return ExitCode::from(70);
    }
    let Mode::Run(args) = mode else { return ExitCode::SUCCESS };

    match supervise(Path::new(FIRECRACKER), &args, io::stdin()) {
        Ok(status) => ExitCode::from(exit_code(status)),
        Err(e) => {
            eprintln!("sandkiln-firecracker-supervisor: cannot run {FIRECRACKER}: {e}");
            ExitCode::from(71)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn self_check_is_recognised_anywhere_in_the_arguments() {
        // jailer appends its own arguments, so the flag is not always first.
        assert_eq!(parse_args(os(&["--self-check", "--id", "x"])), Mode::SelfCheck);
        assert_eq!(parse_args(os(&["--id", "x", "--self-check"])), Mode::SelfCheck);
    }

    #[test]
    fn everything_else_is_passed_to_firecracker_untouched() {
        let args = os(&["--id", "x", "--api-sock", "/api.sock"]);
        assert_eq!(parse_args(args.clone()), Mode::Run(args));
    }

    #[test]
    fn directories_become_group_accessible_with_setgid() {
        let root = std::env::temp_dir().join(format!("sandkiln-supervisor-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dev/net")).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(root.join("dev"), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(root.join("dev/net"), std::fs::Permissions::from_mode(0o700)).unwrap();

        // `run` is absent on purpose: a directory jailer did not create is skipped.
        open_dirs_to_group(&root).unwrap();

        for rel in ["", "dev", "dev/net"] {
            let mode = std::fs::metadata(root.join(rel)).unwrap().permissions().mode();
            assert_eq!(mode & 0o7777, GROUP_DIR_MODE, "{rel:?}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_chroot_root_is_an_error() {
        assert!(open_dirs_to_group(Path::new("/nonexistent-sandkiln-jail-root")).is_err());
    }

    #[test]
    fn closing_the_control_stream_kills_the_child_promptly() {
        let started = Instant::now();
        // An already-closed stream: the daemon went away before anything ran.
        let status = supervise(Path::new("/bin/sleep"), &os(&["60"]), io::empty()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }

    #[test]
    fn a_child_that_exits_on_its_own_reports_its_own_status() {
        // The control stream never closes while the child runs.
        let (reader, _writer) = std::os::unix::net::UnixStream::pair().unwrap();
        let status = supervise(Path::new("/bin/sh"), &os(&["-c", "exit 7"]), reader).unwrap();
        assert_eq!(status.code(), Some(7));
        assert_eq!(exit_code(status), 7);
    }

    #[test]
    fn exit_code_of_a_signalled_child_follows_the_shell_convention() {
        let status = ExitStatus::from_raw(libc::SIGKILL);
        assert_eq!(exit_code(status), 128 + 9);
    }
}

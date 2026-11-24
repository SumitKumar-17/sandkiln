//! Bounded process termination for a VM's child, and the error that says a
//! process could not be ended.
//!
//! `Child::wait` has no deadline, so `kill(); wait()` hangs forever when the
//! kill does not take effect. That is not hypothetical under the jailer: the
//! jailed Firecracker runs as another uid, and the unprivileged daemon's
//! SIGKILL is refused with `EPERM` (finding 32 in `docs/findings.md`). Every
//! teardown here therefore polls with a deadline and reports what it could
//! not do instead of blocking.

use std::fmt;
use std::io;
use std::process::Child;
use std::time::{Duration, Instant};

/// How long a teardown waits for a killed child to be reaped. SIGKILL is
/// delivered immediately, so anything longer than this means the kill did
/// not apply, not that it is slow.
pub(crate) const REAP_GRACE: Duration = Duration::from_secs(5);

/// A VM process that is still alive after a teardown. Carried as the
/// payload of an `io::Error` (see [`is_unkilled_process`]) so the daemon can
/// tell this apart from an ordinary failure: the uid/gid the process runs
/// under must NOT go back into the pool, or the next jailed VM would share
/// an identity with a live, possibly guest-controlled, process.
#[derive(Debug)]
pub struct UnkilledProcess {
    pub pid: u32,
    pub message: String,
}

impl fmt::Display for UnkilledProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UnkilledProcess {}

/// Whether `err` reports a process that could not be killed.
pub fn is_unkilled_process(err: &io::Error) -> bool {
    err.get_ref().is_some_and(|inner| inner.is::<UnkilledProcess>())
}

pub(crate) enum Termination {
    /// The child is gone and reaped.
    Exited,
    /// The child is still running after `grace`; `kill_error` is why the
    /// kill did not take effect, when the kernel said.
    StillRunning { pid: u32, kill_error: Option<io::Error> },
}

/// SIGKILLs `child` and waits at most `grace` for it to be reaped.
pub(crate) fn terminate_child(child: &mut Child, grace: Duration) -> Termination {
    let kill_error = child.kill().err();
    let deadline = Instant::now() + grace;
    loop {
        if let Ok(Some(_)) = child.try_wait() {
            return Termination::Exited;
        }
        if Instant::now() >= deadline {
            return Termination::StillRunning { pid: child.id(), kill_error };
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The error for a process that survived teardown, with the likely cause
/// spelled out for an operator.
pub(crate) fn unkilled_error(pid: u32, kill_error: Option<&io::Error>, context: &str) -> io::Error {
    let reason = match kill_error {
        Some(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            "the kernel refused the kill (EPERM): a jailed Firecracker runs under another uid, which the unprivileged daemon may not signal"
                .to_string()
        }
        Some(e) => format!("the kill failed: {e}"),
        None => "it did not exit after SIGKILL".to_string(),
    };
    io::Error::other(UnkilledProcess {
        pid,
        message: format!("{context}; process {pid} is still running ({reason}); its uid is withheld from reuse until the daemon restarts"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn terminate_child_kills_and_reaps_a_running_process() {
        let mut child = Command::new("sleep").arg("60").spawn().unwrap();
        assert!(matches!(terminate_child(&mut child, Duration::from_secs(5)), Termination::Exited));
    }

    #[test]
    fn terminate_child_is_fine_with_an_already_exited_process() {
        let mut child = Command::new("true").spawn().unwrap();
        child.wait().unwrap();
        assert!(matches!(terminate_child(&mut child, Duration::from_secs(1)), Termination::Exited));
    }

    #[test]
    fn unkilled_error_is_recognisable_and_explains_eperm() {
        let eperm = io::Error::from(io::ErrorKind::PermissionDenied);
        let err = unkilled_error(42, Some(&eperm), "boot failed");
        assert!(is_unkilled_process(&err));
        let text = err.to_string();
        assert!(text.contains("process 42") && text.contains("EPERM") && text.contains("boot failed"), "{text}");
    }

    #[test]
    fn ordinary_errors_are_not_unkilled_process_errors() {
        assert!(!is_unkilled_process(&io::Error::other("plain failure")));
    }
}

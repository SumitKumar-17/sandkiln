//! `Request::Exec` with a `timeout_secs`: run the command in its own process
//! group, wait for it with a deadline, and on expiry kill the *whole group*
//! (so a shell's grandchildren die with it) and report what was captured.
//!
//! The command without a limit does not come through here: `handler::exec`
//! keeps using `Command::output`, unchanged.
//!
//! One channel carries three completion events (the child was reaped, stdout
//! hit EOF, stderr hit EOF) so a single `recv_timeout` loop enforces one
//! deadline over all of them. Waiting on the child alone would not do: a
//! backgrounded grandchild can hold a pipe open after the leader exits, which
//! is exactly the case where `Command::output` would hang too. Both pipes are
//! drained on their own threads for the whole run, so a chatty command can
//! never block on a full pipe while we wait.

use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// How long the group gets after SIGTERM before SIGKILL, and how long we then
/// wait for the child and its pipes to wind down before giving up on them.
const TERM_GRACE: Duration = Duration::from_secs(2);
const KILL_SETTLE: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum Outcome {
    Finished { stdout: Vec<u8>, stderr: Vec<u8>, status: ExitStatus },
    TimedOut { stdout: Vec<u8>, stderr: Vec<u8> },
    /// The command could not be started (missing, not executable, ...). Kept
    /// apart from `Err`, which is a failure of the wait itself, so the caller
    /// can apply the shell exit-code convention to a spawn failure only.
    SpawnFailed(std::io::Error),
}

enum Done {
    Child(std::io::Result<ExitStatus>),
    Stdout,
    Stderr,
}

type SharedBuf = Arc<Mutex<Vec<u8>>>;

pub fn run(command: &str, args: &[String], env: &HashMap<String, String>, timeout: Duration) -> std::io::Result<Outcome> {
    run_with_grace(command, args, env, timeout, TERM_GRACE, KILL_SETTLE)
}

fn run_with_grace(
    command: &str,
    args: &[String],
    env: &HashMap<String, String>,
    timeout: Duration,
    term_grace: Duration,
    kill_settle: Duration,
) -> std::io::Result<Outcome> {
    let deadline = Instant::now() + timeout;
    let spawned = Command::new(command)
        .args(args)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // setpgid(0, 0) in the child: pgid == pid, so the group can be
        // signalled as one and cannot include the agent itself.
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return Ok(Outcome::SpawnFailed(e)),
    };
    let pgid = Pid::from_raw(child.id() as i32);

    let stdout_buf = SharedBuf::default();
    let stderr_buf = SharedBuf::default();
    let (tx, rx) = mpsc::channel::<Done>();
    drain(child.stdout.take().expect("stdout was piped"), stdout_buf.clone(), tx.clone(), Done::Stdout);
    drain(child.stderr.take().expect("stderr was piped"), stderr_buf.clone(), tx.clone(), Done::Stderr);
    let waiter_tx = tx;
    thread::spawn(move || {
        let _ = waiter_tx.send(Done::Child(child.wait()));
    });

    let mut status: Option<std::io::Result<ExitStatus>> = None;
    let mut pending = 3;
    let mut collect = |done: Done, pending: &mut u8| {
        *pending -= 1;
        if let Done::Child(s) = done {
            status = Some(s);
        }
    };

    while pending > 0 {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(done) => collect(done, &mut pending),
            Err(RecvTimeoutError::Timeout) => break,
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    if pending == 0 {
        return match status.expect("child event was counted") {
            Ok(status) => Ok(Outcome::Finished { stdout: take(&stdout_buf), stderr: take(&stderr_buf), status }),
            Err(e) => Err(e),
        };
    }

    // Deadline passed. SIGTERM lets the command clean up; SIGKILL sweeps
    // whatever is left of the group (including processes that ignore TERM,
    // and stragglers once the leader is gone).
    let _ = killpg(pgid, Signal::SIGTERM);
    let term_deadline = Instant::now() + term_grace;
    while pending > 0 {
        match rx.recv_timeout(term_deadline.saturating_duration_since(Instant::now())) {
            Ok(done) => {
                let leader_gone = matches!(done, Done::Child(_));
                collect(done, &mut pending);
                if leader_gone {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = killpg(pgid, Signal::SIGKILL);
    // Bounded: a process that left the group (setsid) can keep a pipe open
    // past the kill; return what was captured rather than hang the agent.
    let settle_deadline = Instant::now() + kill_settle;
    while pending > 0 {
        match rx.recv_timeout(settle_deadline.saturating_duration_since(Instant::now())) {
            Ok(done) => collect(done, &mut pending),
            Err(_) => break,
        }
    }
    Ok(Outcome::TimedOut { stdout: take(&stdout_buf), stderr: take(&stderr_buf) })
}

fn take(buf: &SharedBuf) -> Vec<u8> {
    std::mem::take(&mut *buf.lock().unwrap_or_else(|e| e.into_inner()))
}

fn drain(mut pipe: impl Read + Send + 'static, buf: SharedBuf, tx: mpsc::Sender<Done>, done: Done) {
    thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.lock().unwrap_or_else(|e| e.into_inner()).extend_from_slice(&chunk[..n]),
            }
        }
        let _ = tx.send(done);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, timeout: Duration) -> std::io::Result<Outcome> {
        run("sh", &["-c".to_string(), script.to_string()], &HashMap::new(), timeout)
    }

    fn alive(pid: i32) -> bool {
        // A reaped process has no /proc entry; an unreaped zombie reports Z.
        match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => !stat.rsplit(')').next().unwrap_or("").trim_start().starts_with('Z'),
            Err(_) => false,
        }
    }

    fn wait_gone(pid: i32) -> bool {
        for _ in 0..100 {
            if !alive(pid) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn command_that_finishes_in_time_reports_exit_code_and_output() {
        let Outcome::Finished { stdout, stderr, status } = sh("echo hi; echo err >&2; exit 3", Duration::from_secs(10)).unwrap() else {
            panic!("expected Finished")
        };
        assert_eq!(stdout, b"hi\n");
        assert_eq!(stderr, b"err\n");
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn expiry_kills_the_command_and_returns_partial_output() {
        let started = Instant::now();
        let Outcome::TimedOut { stdout, stderr } = sh("echo partial; echo oops >&2; sleep 30", Duration::from_secs(1)).unwrap() else {
            panic!("expected TimedOut")
        };
        assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
        assert_eq!(stdout, b"partial\n");
        assert_eq!(stderr, b"oops\n");
    }

    #[test]
    fn expiry_kills_grandchildren_in_the_group() {
        let Outcome::TimedOut { stdout, .. } = sh("sleep 60 & echo $!; wait", Duration::from_secs(1)).unwrap() else {
            panic!("expected TimedOut")
        };
        let pid: i32 = String::from_utf8(stdout).unwrap().trim().parse().expect("grandchild pid on stdout");
        assert!(wait_gone(pid), "grandchild {pid} survived the group kill");
    }

    #[test]
    fn background_child_holding_the_pipe_does_not_outlive_the_deadline() {
        // The leader exits at once but its child keeps stdout open: the
        // deadline must still apply (Command::output would block here).
        let started = Instant::now();
        let out = sh("sleep 60 & echo $!", Duration::from_secs(1)).unwrap();
        assert!(matches!(out, Outcome::TimedOut { .. }));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_command_ignoring_sigterm_is_killed_after_the_grace() {
        let started = Instant::now();
        let out = run_with_grace(
            "sh",
            &["-c".to_string(), "trap '' TERM; echo ready; sleep 60".to_string()],
            &HashMap::new(),
            Duration::from_millis(500),
            Duration::from_millis(300),
            Duration::from_secs(2),
        )
        .unwrap();
        let Outcome::TimedOut { stdout, .. } = out else { panic!("expected TimedOut") };
        assert_eq!(stdout, b"ready\n");
        assert!(started.elapsed() < Duration::from_secs(4), "took {:?}", started.elapsed());
    }

    #[test]
    fn chatty_output_is_drained_while_waiting() {
        // 5 MB overflows any pipe buffer; if nothing drained it concurrently
        // the writer would block and we would only learn at the deadline.
        let Outcome::Finished { stdout, .. } = sh("head -c 5000000 /dev/zero", Duration::from_secs(10)).unwrap() else {
            panic!("expected Finished")
        };
        assert_eq!(stdout.len(), 5_000_000);
    }

    #[test]
    fn chatty_output_before_expiry_is_kept() {
        let Outcome::TimedOut { stdout, .. } = sh("head -c 3000000 /dev/zero; sleep 30", Duration::from_secs(2)).unwrap() else {
            panic!("expected TimedOut")
        };
        assert_eq!(stdout.len(), 3_000_000);
    }

    #[test]
    fn unspawnable_command_is_a_spawn_failure_not_a_timeout() {
        let Outcome::SpawnFailed(err) = run("/definitely/not/a/command", &[], &HashMap::new(), Duration::from_secs(1)).unwrap() else {
            panic!("expected SpawnFailed")
        };
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}

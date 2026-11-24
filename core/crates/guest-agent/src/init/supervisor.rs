//! PID 1's loop: reap every child, keep the agent running, and sync then
//! restart the guest on a shutdown signal.
//!
//! The agent is a forked child rather than PID 1 itself so `waitpid(-1)`
//! here only ever sees the agent and orphans reparented to init, never the
//! processes the agent is waiting on (`Command::output`, `forkpty`).

use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// Signal number a handler caught, 0 for none. A lone atomic store is all a
/// handler may safely do.
static PENDING_SIGNAL: AtomicI32 = AtomicI32::new(0);

/// Signals that mean "stop the guest": `kill 1`, `halt`/`poweroff` from
/// sysvinit-style tools, and Ctrl-Alt-Del (see `setup::prepare`).
const SHUTDOWN_SIGNALS: &[libc::c_int] = &[libc::SIGTERM, libc::SIGINT, libc::SIGUSR1, libc::SIGUSR2];

/// A crash-looping agent is retried with a growing delay (capped) so a
/// persistent fault cannot spin the guest CPU or flood the console.
pub(super) fn restart_delay(consecutive_fast_failures: u32) -> Duration {
    if consecutive_fast_failures == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(50u64 << consecutive_fast_failures.min(5).saturating_sub(1)).min(Duration::from_secs(1))
}

/// An agent that stayed up this long counts as healthy, resetting the backoff.
const HEALTHY_AFTER: Duration = Duration::from_secs(10);

/// Next failure count: fast exits accumulate, a long run resets.
pub(super) fn next_failures(previous: u32, ran_for: Duration) -> u32 {
    if ran_for >= HEALTHY_AFTER { 1 } else { previous.saturating_add(1) }
}

extern "C" fn on_signal(sig: libc::c_int) {
    PENDING_SIGNAL.store(sig, Ordering::SeqCst);
}

fn install_handlers() {
    for &sig in SHUTDOWN_SIGNALS {
        // SAFETY: the handler only does an atomic store. No SA_RESTART, so
        // a blocked waitpid returns EINTR and the loop notices the signal.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(sig, &action, std::ptr::null_mut());
        }
    }
}

/// In the forked child: the agent must see default signal behaviour and an
/// empty mask, not PID 1's handlers.
fn reset_signals_for_child() {
    for &sig in SHUTDOWN_SIGNALS {
        // SAFETY: restoring SIG_DFL is always valid.
        unsafe { libc::signal(sig, libc::SIG_DFL) };
    }
}

fn spawn_agent(agent: fn() -> !) -> libc::pid_t {
    // SAFETY: PID 1 is single-threaded here, so fork is safe; the child only
    // resets signals and enters the agent, which starts its own threads.
    match unsafe { libc::fork() } {
        0 => {
            reset_signals_for_child();
            agent()
        }
        pid if pid > 0 => pid,
        _ => {
            eprintln!("init: fork agent: {}", std::io::Error::last_os_error());
            -1
        }
    }
}

fn shutdown() -> ! {
    eprintln!("init: shutting down");
    // SAFETY: sync and reboot take no pointers. Restart, not power-off:
    // Firecracker exits on a guest reset (kernel arg `reboot=k`) and has no
    // ACPI power-off path.
    unsafe {
        libc::sync();
        libc::reboot(libc::LINUX_REBOOT_CMD_RESTART);
    }
    // reboot(2) only returns on failure; PID 1 must not exit.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

pub(super) fn supervise(agent: fn() -> !) -> ! {
    install_handlers();
    let mut agent_pid = spawn_agent(agent);
    let mut started = Instant::now();
    let mut failures = 0u32;

    loop {
        if PENDING_SIGNAL.load(Ordering::SeqCst) != 0 {
            shutdown();
        }
        let mut status: libc::c_int = 0;
        // SAFETY: `status` is a valid out-pointer.
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        let agent_gone = if pid < 0 {
            // EINTR: a signal arrived, recheck at the top. ECHILD: no
            // children at all, which only happens if the agent is gone.
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EINTR) => continue,
                _ => true,
            }
        } else {
            pid == agent_pid
        };
        if !agent_gone && agent_pid >= 0 {
            continue; // an orphan reaped, nothing else to do
        }
        failures = next_failures(failures, started.elapsed());
        eprintln!("init: agent exited (status {status}), restarting");
        std::thread::sleep(restart_delay(failures.saturating_sub(1)));
        agent_pid = spawn_agent(agent);
        started = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_restart_is_immediate_then_backs_off_to_a_cap() {
        assert_eq!(restart_delay(0), Duration::ZERO);
        assert_eq!(restart_delay(1), Duration::from_millis(50));
        assert_eq!(restart_delay(2), Duration::from_millis(100));
        assert_eq!(restart_delay(5), Duration::from_millis(800));
        assert_eq!(restart_delay(50), Duration::from_millis(800));
    }

    #[test]
    fn fast_failures_accumulate_and_a_healthy_run_resets() {
        assert_eq!(next_failures(0, Duration::from_millis(5)), 1);
        assert_eq!(next_failures(3, Duration::from_secs(1)), 4);
        assert_eq!(next_failures(9, HEALTHY_AFTER), 1);
        assert_eq!(next_failures(u32::MAX, Duration::ZERO), u32::MAX);
    }

    #[test]
    fn every_shutdown_signal_is_catchable() {
        assert!(!SHUTDOWN_SIGNALS.contains(&libc::SIGKILL) && !SHUTDOWN_SIGNALS.contains(&libc::SIGSTOP));
    }
}

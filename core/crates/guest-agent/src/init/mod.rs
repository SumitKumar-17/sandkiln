//! Init mode: the agent as the guest's PID 1, in place of a general init.
//!
//! Selected purely by `getpid() == 1`, so the same binary stays a plain
//! agent under any other init and a stray local run can never mount over a
//! host. A rootfs opts in per image by pointing `/sbin/init` at the agent
//! (`images/inject-agent.sh --init`); no daemon or kernel-argument change.
//!
//! - [`setup`]: one-shot guest preparation (pseudo filesystems, hostname,
//!   loopback, environment, limits) matching what systemd gave the agent.
//! - [`supervisor`]: PID 1's job afterwards. The agent runs as a forked
//!   child, so PID 1 can reap with `waitpid(-1)` without stealing exit
//!   statuses from the agent's own children (only orphans reparent here).

mod setup;
mod supervisor;

/// True when this process is the guest's init.
pub fn is_init_process() -> bool {
    std::process::id() == 1
}

/// Prepares the guest and supervises `agent` forever. Never returns: a PID 1
/// that exits panics the kernel, so even an unrecoverable setup error falls
/// through to supervising.
pub fn run(agent: fn() -> !) -> ! {
    setup::prepare();
    supervisor::supervise(agent)
}

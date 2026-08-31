//! Background task that reclaims sandboxes that have gone idle: auto-
//! suspends them (pause + snapshot, keeping state resumable — see
//! `crate::routes_snapshot::snapshot_sandbox_by_id`) past
//! `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`, and/or destroys them outright
//! (VM killed, network lease released, rootfs deleted — see
//! `crate::routes_sandbox::stop_sandbox_by_id`) past
//! `SANDKILN_IDLE_TIMEOUT_SECS` (see `config::Config`'s doc comments on
//! both fields for how the two interact when both are configured). Only
//! spawned by `main` when at least one of the two is configured —
//! otherwise sandboxes run until explicitly stopped, same as before either
//! existed.

use crate::routes_sandbox::{stop_sandbox_by_id, StopError};
use crate::routes_snapshot::{snapshot_and_stop, SnapshotBlocked, SnapshotStopError};
use crate::state::AppState;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Checking more often than the shortest configured timeout wastes work;
/// checking only once per timeout risks a sandbox running up to ~2x the
/// configured window before being caught. Splitting the difference, capped
/// so a huge configured timeout doesn't make the loop check absurdly
/// rarely.
const MAX_CHECK_INTERVAL: Duration = Duration::from_secs(30);

pub async fn run(state: Arc<AppState>, idle_timeout: Option<Duration>, auto_suspend_timeout: Option<Duration>) {
    let configured_timeouts: Vec<Duration> = [idle_timeout, auto_suspend_timeout].into_iter().flatten().collect();
    let check_interval = compute_check_interval(&configured_timeouts);
    loop {
        tokio::time::sleep(check_interval).await;
        reap_once(&state, idle_timeout, auto_suspend_timeout, Instant::now()).await;
    }
}

/// One reaper tick. Auto-suspend runs first: a sandbox it successfully
/// suspends leaves `AppState::sandboxes` entirely (it's a `Snapshot` now),
/// so the destroy pass below naturally never sees it again — see
/// `config::Config::auto_suspend_timeout`'s doc comment for why this
/// ordering, plus the required `auto_suspend_timeout < idle_timeout`
/// invariant enforced at startup, is what makes destroy a backstop rather
/// than a race.
async fn reap_once(state: &Arc<AppState>, idle_timeout: Option<Duration>, auto_suspend_timeout: Option<Duration>, now: Instant) {
    if let Some(suspend_timeout) = auto_suspend_timeout {
        suspend_idle_sandboxes(state, suspend_timeout, now).await;
    }
    if let Some(destroy_timeout) = idle_timeout {
        destroy_idle_sandboxes(state, destroy_timeout, now).await;
    }
}

async fn suspend_idle_sandboxes(state: &Arc<AppState>, timeout: Duration, now: Instant) {
    for id in idle_sandbox_ids(state, timeout, now) {
        tracing::info!(sandbox_id = %id, "auto-suspending idle sandbox");
        match snapshot_and_stop(state.clone(), id.clone()).await {
            Ok(snapshot_id) => {
                tracing::info!(sandbox_id = %id, snapshot_id = %snapshot_id, "auto-suspended idle sandbox");
            }
            Err(SnapshotStopError::NotFound) => {
                // Only realistic cause: it was already removed (raced with
                // a concurrent explicit stop/snapshot) between the scan
                // above and here — same tolerance `destroy_idle_sandboxes`
                // already has for the destroy path.
                tracing::warn!(sandbox_id = %id, "idle sandbox was already gone by the time the reaper tried to auto-suspend it");
            }
            Err(SnapshotStopError::Blocked(reason)) => {
                // Structurally ineligible for suspend (booted jailed, or
                // forked from a snapshot and sharing its rootfs — see
                // `snapshot_and_stop`'s own precondition checks), not an
                // operational failure. Left running: it'll be scanned
                // again next tick and log this again until either it goes
                // idle-active again or `SANDKILN_IDLE_TIMEOUT_SECS`, if
                // configured, eventually destroys it instead. `debug`
                // rather than `warn` specifically because this can repeat
                // every tick for as long as such a sandbox stays idle.
                let reason: &str = match reason {
                    SnapshotBlocked::Jailed => "jailed",
                    SnapshotBlocked::ForkedFrom(_) => "forked from another snapshot",
                };
                tracing::debug!(sandbox_id = %id, reason, "sandbox is idle but not eligible for auto-suspend — leaving it running");
            }
            Err(SnapshotStopError::Io(e)) => {
                // A real failure partway through pause/snapshot (disk
                // full, a Firecracker API error, a metadata-persist
                // failure) — `snapshot_and_stop` itself already degrades
                // this the same way the manual `POST .../snapshot` route
                // does: stop the VM and release its resources rather than
                // hand back something claiming to still be a live,
                // running sandbox. There's no primitive to un-pause a VM
                // once Firecracker's `/vm` PATCH to `Paused` has taken
                // effect, so "leave it running and retry" isn't actually
                // available once pause has succeeded — the sandbox is
                // gone either way by the time this arm runs, same net
                // effect as an idle-destroy, just logged distinctly so an
                // operator can tell the difference between "reclaimed on
                // purpose" and "auto-suspend broke".
                tracing::warn!(
                    sandbox_id = %id,
                    error = %e,
                    "auto-suspend failed for idle sandbox — it was stopped and its resources released as a fallback \
                     rather than left half-paused; it will not be retried"
                );
            }
        }
    }
}

async fn destroy_idle_sandboxes(state: &Arc<AppState>, timeout: Duration, now: Instant) {
    for id in idle_sandbox_ids(state, timeout, now) {
        tracing::info!(sandbox_id = %id, "stopping idle sandbox");
        // `keep: true` — same "preserve by default" behavior as an
        // explicit `DELETE /sandboxes/:id`, via the exact same shared
        // path (see `stop_sandbox_by_id`'s doc comment): an idle-timeout
        // stop shouldn't discard state a caller would keep if they'd
        // stopped it themselves.
        match stop_sandbox_by_id(state.clone(), id.clone(), true).await {
            Ok(_) => {}
            Err(StopError::NotFound) => {
                // Only realistic cause: it was already removed (raced with
                // a concurrent explicit stop, or already auto-suspended
                // above in this same tick) between the scan above and here.
                tracing::warn!(sandbox_id = %id, "idle sandbox was already gone by the time the reaper tried to stop it");
            }
            Err(StopError::CannotPreserve(_)) => {
                // Preservation is structurally impossible for this
                // sandbox (jailed — see `SnapshotBlocked`), and unlike the
                // `DELETE` route there's no caller here to redirect
                // toward `?keep=false`: leaving it running forever would
                // just leak its VM/network resources. Free them instead,
                // same as an explicit destroy would.
                tracing::warn!(
                    sandbox_id = %id,
                    "idle sandbox cannot be preserved on stop (unsupported for this sandbox) — destroying it instead to free its resources"
                );
                if let Err(_e) = stop_sandbox_by_id(state.clone(), id.clone(), false).await {
                    tracing::warn!(sandbox_id = %id, "fallback destroy of an unpreservable idle sandbox also failed");
                }
            }
            Err(StopError::Io(e)) => {
                // `snapshot_and_stop` already tore the VM down on this
                // path (see its doc comment: "whether or not the snapshot
                // succeeded, this VM is done") — nothing further to clean
                // up here, just a data-loss signal worth logging loudly.
                tracing::warn!(sandbox_id = %id, error = %e, "idle sandbox's snapshot-on-stop failed — its state was not preserved");
            }
        }
    }
}

fn idle_sandbox_ids(state: &Arc<AppState>, timeout: Duration, now: Instant) -> Vec<String> {
    let sandboxes = state.sandboxes.lock().unwrap();
    sandboxes
        .iter()
        .filter(|(_, sandbox)| is_idle(*sandbox.last_activity.lock().unwrap(), now, timeout))
        .map(|(id, _)| id.clone())
        .collect()
}

/// Pure decision logic, pulled out of the scan/stop plumbing above so it's
/// directly testable without a real `AppState`/`Sandbox` — same pattern as
/// `auth::token_matches`.
fn is_idle(last_activity: Instant, now: Instant, timeout: Duration) -> bool {
    now.saturating_duration_since(last_activity) >= timeout
}

/// Picks how often the reaper wakes to scan, based on the shortest of
/// whichever timeouts are actually configured — same halve-and-clamp
/// reasoning as when there was only ever one timeout to consider, just
/// generalized to more than one independent threshold. `run` only ever
/// calls this with at least one configured timeout (`main` only spawns
/// the reaper at all when that holds), so the empty case here only matters
/// for this function's own testability in isolation.
fn compute_check_interval(configured_timeouts: &[Duration]) -> Duration {
    match configured_timeouts.iter().copied().min() {
        Some(shortest) => (shortest / 2).clamp(Duration::from_secs(1), MAX_CHECK_INTERVAL),
        None => MAX_CHECK_INTERVAL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_when_elapsed_meets_timeout_exactly() {
        let now = Instant::now();
        let last_activity = now - Duration::from_secs(60);
        assert!(is_idle(last_activity, now, Duration::from_secs(60)));
    }

    #[test]
    fn idle_when_elapsed_exceeds_timeout() {
        let now = Instant::now();
        let last_activity = now - Duration::from_secs(120);
        assert!(is_idle(last_activity, now, Duration::from_secs(60)));
    }

    #[test]
    fn not_idle_when_elapsed_under_timeout() {
        let now = Instant::now();
        let last_activity = now - Duration::from_secs(10);
        assert!(!is_idle(last_activity, now, Duration::from_secs(60)));
    }

    #[test]
    fn not_idle_immediately_after_activity() {
        let now = Instant::now();
        assert!(!is_idle(now, now, Duration::from_secs(60)));
    }

    #[test]
    fn check_interval_halves_the_single_configured_timeout() {
        assert_eq!(compute_check_interval(&[Duration::from_secs(10)]), Duration::from_secs(5));
    }

    #[test]
    fn check_interval_uses_the_shortest_of_multiple_configured_timeouts() {
        assert_eq!(
            compute_check_interval(&[Duration::from_secs(600), Duration::from_secs(20)]),
            Duration::from_secs(10)
        );
    }

    #[test]
    fn check_interval_is_clamped_to_at_least_one_second() {
        assert_eq!(compute_check_interval(&[Duration::from_millis(500)]), Duration::from_secs(1));
    }

    #[test]
    fn check_interval_is_clamped_to_the_maximum() {
        assert_eq!(compute_check_interval(&[Duration::from_secs(3600)]), MAX_CHECK_INTERVAL);
    }
}

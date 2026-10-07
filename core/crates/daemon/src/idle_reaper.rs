//! Background task that reclaims idle sandboxes and archives old held
//! snapshots. Full tiered-lifecycle design is written up in
//! `docs/architecture/02-vm-boot-and-latency.md`; the invariants a reader
//! editing this file needs: auto-suspend (pause + snapshot, see
//! `crate::routes_snapshot::snapshot_sandbox_by_id`, past
//! `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`) must run before destroy (past
//! `SANDKILN_IDLE_TIMEOUT_SECS`, see `config::Config`'s doc comments for
//! how the two interact) each tick; archiving (past
//! `SANDKILN_ARCHIVE_TIMEOUT_SECS`, via
//! `crate::routes_snapshot::archive_snapshot_by_id`) runs regardless of
//! whether either sandbox-side timeout is configured, since it's about a
//! snapshot's own age, not a live sandbox's idle time; and this task
//! spawns unconditionally from `main` — same no-op-tick-is-cheap
//! reasoning `pool_replenisher` already uses.

use crate::routes_sandbox::{stop_sandbox_by_id, StopError};
use crate::routes_snapshot::{archive_snapshot_by_id, snapshot_and_stop, ArchiveError, SnapshotBlocked, SnapshotStopError};
use crate::state::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Halving the shortest configured timeout balances "catches idle
/// sandboxes promptly" against "don't scan needlessly often"; this caps
/// how rarely that halving can land for a huge configured timeout.
const MAX_CHECK_INTERVAL: Duration = Duration::from_secs(30);

pub async fn run(
    state: Arc<AppState>,
    idle_timeout: Option<Duration>,
    auto_suspend_timeout: Option<Duration>,
    archive_timeout: Option<Duration>,
    archive_dir: PathBuf,
) {
    let configured_timeouts: Vec<Duration> = [idle_timeout, auto_suspend_timeout, archive_timeout].into_iter().flatten().collect();
    let check_interval = compute_check_interval(&configured_timeouts);
    loop {
        tokio::time::sleep(check_interval).await;
        reap_once(&state, idle_timeout, auto_suspend_timeout, Instant::now()).await;
        if let Some(archive_timeout) = archive_timeout {
            archive_idle_snapshots(&state, archive_timeout, &archive_dir, SystemTime::now()).await;
        }
    }
}

/// Auto-suspend runs first: a successfully-suspended sandbox leaves
/// `AppState::sandboxes` (it's a `Snapshot` now), so destroy below never
/// sees it — this ordering plus the enforced
/// `auto_suspend_timeout < idle_timeout` invariant is what makes destroy
/// a backstop, not a race (see `config::Config::auto_suspend_timeout`).
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
                // Raced with a concurrent explicit stop/snapshot between
                // the scan and here.
                tracing::warn!(sandbox_id = %id, "idle sandbox was already gone by the time the reaper tried to auto-suspend it");
            }
            Err(SnapshotStopError::Blocked(reason)) => {
                // Structurally ineligible (jailed, or forked and sharing
                // its rootfs), not an operational failure -- left running,
                // rescanned next tick. `debug`, not `warn`: can repeat
                // every tick for as long as it stays idle.
                let reason: &str = match reason {
                    SnapshotBlocked::Jailed => "jailed",
                    SnapshotBlocked::ForkedFrom(_) => "forked from another snapshot",
                };
                tracing::debug!(sandbox_id = %id, reason, "sandbox is idle but not eligible for auto-suspend — leaving it running");
            }
            Err(SnapshotStopError::Io(e)) => {
                // Real failure mid pause/snapshot -- `snapshot_and_stop`
                // already degrades to stop+release (no primitive to
                // un-pause a VM once Firecracker's Paused PATCH lands), so
                // the sandbox is gone either way; logged distinctly so an
                // operator can tell "reclaimed on purpose" from
                // "auto-suspend broke".
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
        // keep: true -- same preserve-by-default behavior as an explicit
        // DELETE, via the same shared path.
        match stop_sandbox_by_id(state.clone(), id.clone(), true).await {
            Ok(_) => {}
            Err(StopError::NotFound) => {
                // Raced with a concurrent stop, or already auto-suspended
                // above this same tick.
                tracing::warn!(sandbox_id = %id, "idle sandbox was already gone by the time the reaper tried to stop it");
            }
            Err(StopError::CannotPreserve(_)) => {
                // Structurally impossible to preserve (jailed), and unlike
                // DELETE there's no caller to redirect to ?keep=false --
                // free the resources instead of leaking them forever.
                tracing::warn!(
                    sandbox_id = %id,
                    "idle sandbox cannot be preserved on stop (unsupported for this sandbox) — destroying it instead to free its resources"
                );
                if let Err(_e) = stop_sandbox_by_id(state.clone(), id.clone(), false).await {
                    tracing::warn!(sandbox_id = %id, "fallback destroy of an unpreservable idle sandbox also failed");
                }
            }
            Err(StopError::Io(e)) => {
                // snapshot_and_stop already tore the VM down on this path
                // -- nothing left to clean up, just a loud data-loss signal.
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

/// Archive tier: moves eligible held snapshots' files onto
/// `Config::archive_dir` (`archive_snapshot_by_id`). Eligible = not
/// already archived, no live fork (a fork's `Vm::resume` is actively
/// using the current file paths), and old enough. Looks at
/// `AppState::snapshots`, independent of the sandbox-side passes above.
async fn archive_idle_snapshots(state: &Arc<AppState>, timeout: Duration, archive_dir: &std::path::Path, now: SystemTime) {
    for id in due_for_archive_ids(state, timeout, now) {
        tracing::info!(snapshot_id = %id, "archiving idle snapshot");
        match archive_snapshot_by_id(state.clone(), id.clone(), archive_dir.to_path_buf()).await {
            Ok(()) => {
                tracing::info!(snapshot_id = %id, "archived idle snapshot");
            }
            Err(ArchiveError::NotFound) => {
                // Resumed, forked, deleted, or already archived between
                // the scan and here.
                tracing::warn!(snapshot_id = %id, "idle snapshot was already gone by the time the reaper tried to archive it");
            }
            Err(ArchiveError::Forked) => {
                // Forked concurrently, after the scan's own filter passed
                // -- left alone rather than archived out from under the
                // fork; retried next tick.
                tracing::debug!(snapshot_id = %id, "idle snapshot gained a live fork before it could be archived — leaving it alone");
            }
            Err(ArchiveError::Io(e)) => {
                // Can leave a genuinely mixed set of old/new file paths
                // (see `move_snapshot_files`) -- a loud signal this one
                // needs manual attention, not silent retry.
                tracing::warn!(snapshot_id = %id, error = %e, "failed to archive an idle snapshot — its files may now be split across the hot and archive directories");
            }
        }
    }
}

fn due_for_archive_ids(state: &Arc<AppState>, timeout: Duration, now: SystemTime) -> Vec<String> {
    let snapshots = state.snapshots.lock().unwrap();
    snapshots
        .values()
        .filter(|snapshot| snapshot.archived_at.is_none() && snapshot.forked_into.is_none())
        .filter(|snapshot| is_archive_due(snapshot.created_at, now, timeout))
        .map(|snapshot| snapshot.id.clone())
        .collect()
}

/// `SystemTime`, not `Instant`: `Snapshot::created_at` persists as a unix
/// timestamp across restarts, which `Instant` can't do. `created_at`
/// somehow after `now` (clock skew) reads as "not due" rather than panic.
fn is_archive_due(created_at: SystemTime, now: SystemTime, timeout: Duration) -> bool {
    now.duration_since(created_at).is_ok_and(|elapsed| elapsed >= timeout)
}

/// Pulled out of the scan/stop plumbing so it's directly testable, same
/// pattern as `auth::token_matches`.
fn is_idle(last_activity: Instant, now: Instant, timeout: Duration) -> bool {
    now.saturating_duration_since(last_activity) >= timeout
}

/// Halves the shortest configured timeout, clamped. `run` spawns
/// unconditionally, so the nothing-configured case (`MAX_CHECK_INTERVAL`)
/// is common in practice, not just a testability nicety.
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

    #[test]
    fn archive_due_when_elapsed_meets_timeout_exactly() {
        let now = SystemTime::now();
        let created_at = now - Duration::from_secs(60);
        assert!(is_archive_due(created_at, now, Duration::from_secs(60)));
    }

    #[test]
    fn archive_due_when_elapsed_exceeds_timeout() {
        let now = SystemTime::now();
        let created_at = now - Duration::from_secs(120);
        assert!(is_archive_due(created_at, now, Duration::from_secs(60)));
    }

    #[test]
    fn archive_not_due_when_elapsed_under_timeout() {
        let now = SystemTime::now();
        let created_at = now - Duration::from_secs(10);
        assert!(!is_archive_due(created_at, now, Duration::from_secs(60)));
    }

    #[test]
    fn archive_not_due_immediately_after_creation() {
        let now = SystemTime::now();
        assert!(!is_archive_due(now, now, Duration::from_secs(60)));
    }

    #[test]
    fn archive_not_due_when_created_at_is_somehow_after_now() {
        // Clock skew, or the two racing within the same instant --
        // `duration_since` returns `Err` here, which must read as "not
        // due yet", not panic or silently treat it as a huge duration.
        let now = SystemTime::now();
        let created_at = now + Duration::from_secs(5);
        assert!(!is_archive_due(created_at, now, Duration::from_secs(1)));
    }
}

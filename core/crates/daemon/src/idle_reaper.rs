//! Background task that reclaims sandboxes that have gone idle: auto-
//! suspends them (pause + snapshot, keeping state resumable — see
//! `crate::routes_snapshot::snapshot_sandbox_by_id`) past
//! `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`, and/or destroys them outright
//! (VM killed, network lease released, rootfs deleted — see
//! `crate::routes_sandbox::stop_sandbox_by_id`) past
//! `SANDKILN_IDLE_TIMEOUT_SECS` (see `config::Config`'s doc comments on
//! both fields for how the two interact when both are configured).
//!
//! Also runs the tiered lifecycle's next step past suspend: archiving a
//! held *snapshot* (however it arose — auto-suspend or a manual
//! `POST /sandboxes/:id/snapshot`) past `SANDKILN_ARCHIVE_TIMEOUT_SECS`,
//! moving its files off `snapshots_root()` onto `Config::archive_dir` (see
//! `crate::routes_snapshot::archive_snapshot_by_id`). Independent of the
//! two sandbox-side timeouts above — it's about a snapshot's own age, not
//! a live sandbox's idle time, so it runs whether or not either of those
//! is even configured.
//!
//! Spawned unconditionally by `main` (not gated on any of the three being
//! configured) — an idle tick where none apply is a cheap no-op scan, the
//! same reasoning `pool_replenisher` already uses for its own
//! unconditional spawn.

use crate::routes_sandbox::{stop_sandbox_by_id, StopError};
use crate::routes_snapshot::{archive_snapshot_by_id, snapshot_and_stop, ArchiveError, SnapshotBlocked, SnapshotStopError};
use crate::state::AppState;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Checking more often than the shortest configured timeout wastes work;
/// checking only once per timeout risks a sandbox running up to ~2x the
/// configured window before being caught. Splitting the difference, capped
/// so a huge configured timeout doesn't make the loop check absurdly
/// rarely.
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

/// The archive tier: moves every eligible held snapshot's files off
/// `snapshots_root()` onto `Config::archive_dir` — see
/// `crate::routes_snapshot::archive_snapshot_by_id`. Eligible means: not
/// already archived (`archived_at.is_none()`), no live fork
/// (`forked_into.is_none()` — a fork's `Vm::resume` call is actively using
/// this snapshot's *current* file paths right now), and old enough
/// (`is_archive_due`). Independent of the sandbox-side passes above —
/// this looks at `AppState::snapshots`, not `AppState::sandboxes`, and
/// applies to a snapshot regardless of how it came to exist.
async fn archive_idle_snapshots(state: &Arc<AppState>, timeout: Duration, archive_dir: &std::path::Path, now: SystemTime) {
    for id in due_for_archive_ids(state, timeout, now) {
        tracing::info!(snapshot_id = %id, "archiving idle snapshot");
        match archive_snapshot_by_id(state.clone(), id.clone(), archive_dir.to_path_buf()).await {
            Ok(()) => {
                tracing::info!(snapshot_id = %id, "archived idle snapshot");
            }
            Err(ArchiveError::NotFound) => {
                // Only realistic cause: it was resumed, forked, deleted,
                // or already archived by something else between the scan
                // above and here.
                tracing::warn!(snapshot_id = %id, "idle snapshot was already gone by the time the reaper tried to archive it");
            }
            Err(ArchiveError::Forked) => {
                // A fork started concurrently, after the scan's own
                // `forked_into.is_none()` filter already passed — rare,
                // and correctly left alone rather than archived out from
                // under the fork now using it. Picked up again next tick
                // if the fork ends before this snapshot is otherwise
                // resumed/deleted.
                tracing::debug!(snapshot_id = %id, "idle snapshot gained a live fork before it could be archived — leaving it alone");
            }
            Err(ArchiveError::Io(e)) => {
                // See `crate::snapshot::move_snapshot_files`'s own doc
                // comment: a failure here can leave the snapshot with a
                // genuinely mixed set of old/new file paths, already
                // reinserted into `AppState::snapshots` exactly as-is by
                // `archive_snapshot_by_id` — a real, loud signal that
                // this one needs manual attention, not silently retried
                // every tick.
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

/// Pure decision logic, mirroring `is_idle`'s own separation from the
/// scan/archive plumbing above. `SystemTime`, not `Instant`, since
/// `Snapshot::created_at` has to survive a daemon restart (persisted as a
/// unix timestamp) — `Instant` can't be compared across process
/// lifetimes, let alone serialized. A `created_at` somehow after `now`
/// (clock skew, or the two racing within the same instant) is treated as
/// "not due yet" rather than a panic or a nonsensical negative duration.
fn is_archive_due(created_at: SystemTime, now: SystemTime, timeout: Duration) -> bool {
    now.duration_since(created_at).is_ok_and(|elapsed| elapsed >= timeout)
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
/// generalized to more than one independent threshold. `run` is spawned
/// unconditionally now (see this module's own doc comment), so the empty
/// case (nothing configured at all, `MAX_CHECK_INTERVAL`) is a real,
/// common case in practice, not just a testability nicety.
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

//! Background task that stops sandboxes that have gone idle longer than
//! `SANDKILN_IDLE_TIMEOUT_SECS` (see `config::Config::idle_timeout`). Only
//! spawned by `main` when that's configured — otherwise sandboxes keep
//! running until explicitly stopped, same as before this existed.

use crate::routes_sandbox::{stop_sandbox_by_id, StopError};
use crate::state::AppState;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Checking more often than the configured timeout wastes work; checking
/// only once per timeout risks a sandbox running up to ~2x the configured
/// window before being caught. Splitting the difference, capped so a huge
/// configured timeout doesn't make the loop check absurdly rarely.
const MAX_CHECK_INTERVAL: Duration = Duration::from_secs(30);

pub async fn run(state: Arc<AppState>, timeout: Duration) {
    let check_interval = (timeout / 2).clamp(Duration::from_secs(1), MAX_CHECK_INTERVAL);
    loop {
        tokio::time::sleep(check_interval).await;
        reap_once(&state, timeout, Instant::now()).await;
    }
}

async fn reap_once(state: &Arc<AppState>, timeout: Duration, now: Instant) {
    let idle_ids: Vec<String> = {
        let sandboxes = state.sandboxes.lock().unwrap();
        sandboxes
            .iter()
            .filter(|(_, sandbox)| is_idle(*sandbox.last_activity.lock().unwrap(), now, timeout))
            .map(|(id, _)| id.clone())
            .collect()
    };

    for id in idle_ids {
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
                // a concurrent explicit stop) between the scan above and
                // here.
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

/// Pure decision logic, pulled out of the scan/stop plumbing above so it's
/// directly testable without a real `AppState`/`Sandbox` — same pattern as
/// `auth::token_matches`.
fn is_idle(last_activity: Instant, now: Instant, timeout: Duration) -> bool {
    now.saturating_duration_since(last_activity) >= timeout
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
}

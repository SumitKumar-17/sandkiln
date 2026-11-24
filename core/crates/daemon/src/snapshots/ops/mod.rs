//! Snapshot operations: the lifecycle mechanics behind the snapshot routes,
//! the idle reaper, pools and shutdown, one file per operation. Each is
//! defined exactly once here and reused by every caller, so the routes stay
//! thin and the callers cannot drift.
//!
//! - `take`: pause a live sandbox and turn it into a held `Snapshot`.
//! - `resume`: consume a held snapshot into a new sandbox (plus `resume_vm`).
//! - `fork`: boot a new sandbox from a snapshot without consuming it.
//! - `delete` / `archive`: tear a snapshot down, or move it to cold storage.

pub mod archive;
pub mod delete;
pub mod fork;
pub mod resume;
pub mod take;

use crate::http::error::AppError;

/// Pure decision behind every guard here: an operation needing exclusive
/// use of a snapshot's shared resources (fork, consuming resume,
/// deletion) may proceed only while no earlier fork is still alive.
/// Pulled out for direct unit testing.
fn check_no_live_fork(forked_into: Option<&str>, snapshot_id: &str, action: &str) -> Result<(), AppError> {
    match forked_into {
        Some(holder) => Err(AppError::Conflict(format!(
            "snapshot {snapshot_id} has a live fork ({holder}) — stop it before {action} this snapshot"
        ))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_live_fork_allows_the_operation() {
        assert!(check_no_live_fork(None, "snap-1", "forking").is_ok());
    }

    #[test]
    fn a_live_fork_blocks_forking_with_a_clear_message() {
        let err = check_no_live_fork(Some("sbx-9"), "snap-1", "forking").unwrap_err();
        let AppError::Conflict(message) = err else { panic!("expected Conflict, got a different AppError variant") };
        assert!(message.contains("snap-1"), "message was: {message}");
        assert!(message.contains("sbx-9"), "message was: {message}");
        assert!(message.contains("forking"), "message was: {message}");
    }

    #[test]
    fn a_live_fork_blocks_resuming_and_deleting_too() {
        assert!(check_no_live_fork(Some("sbx-9"), "snap-1", "resuming").is_err());
        assert!(check_no_live_fork(Some("sbx-9"), "snap-1", "deleting").is_err());
    }
}

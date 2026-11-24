//! Archive a held snapshot: move its files to the cold archive directory.
//!
//! Owns `archive_snapshot_by_id` and `ArchiveError`. The snapshot record is
//! always reinserted afterwards, success or failure, so a failed archive never
//! loses track of it.

use crate::state::AppState;
use crate::tracing_util::spawn_blocking_in_current_span;
use std::path::PathBuf;
use std::sync::Arc;

/// Every way `archive_snapshot_by_id` can fail.
pub(crate) enum ArchiveError {
    NotFound,
    /// A live fork exists — same exclusion as resume/fork/delete, since
    /// its `Vm::resume` reopens this snapshot's *current* paths.
    Forked,
    Io(std::io::Error),
}

/// Moves an already-held, unforked snapshot's files to `archive_root`
/// via `crate::snapshots::held::move_snapshot_files`. Only called from
/// `idle_reaper`'s archive pass today — no on-demand archive route yet
/// (deliberately deferred, same precedent as `crate::lifecycle::pool`'s narrower
/// first slice).
///
/// Removed from `AppState::snapshots` for the duration of the move (same
/// reasoning as delete/resume: nothing else should touch it mid-move)
/// and **always** reinserted after, success or failure — a failed
/// archive must never lose track of it. See
/// `crate::snapshots::held::move_snapshot_files` for why a failure can leave a
/// genuinely mixed set of old/new paths, reinserted as-is for later
/// inspection rather than guessed at.
pub(crate) async fn archive_snapshot_by_id(state: Arc<AppState>, id: String, archive_root: PathBuf) -> Result<(), ArchiveError> {
    let snapshot = {
        let mut snapshots = state.snapshots.lock().unwrap();
        let existing = snapshots.get(&id).ok_or(ArchiveError::NotFound)?;
        if existing.forked_into.is_some() {
            return Err(ArchiveError::Forked);
        }
        snapshots.remove(&id).expect("just checked it exists")
    };

    // Captured before the move mutates `snapshot.snapshot_path` — once
    // archived, this old dir has only a stale `meta.json` left, which
    // would otherwise make a future restart's `reconcile()` log a
    // false-alarm warning for a healthy, successfully-archived snapshot.
    let old_dir = snapshot.snapshot_path.parent().map(|p| p.to_path_buf());

    let dest_dir = crate::snapshots::held::archive_snapshot_dir(&archive_root, &id);
    let (snapshot, result) = spawn_blocking_in_current_span("archive snapshot task panicked", move || {
        let mut snapshot = snapshot;
        let result = crate::snapshots::held::move_snapshot_files(&mut snapshot, &dest_dir).and_then(|()| {
            snapshot.archived_at = Some(std::time::SystemTime::now());
            snapshot.persist(&dest_dir)
        });
        if result.is_ok() {
            if let Some(old_dir) = old_dir {
                let _ = std::fs::remove_dir_all(old_dir);
            }
        }
        (snapshot, result)
    })
    .await;

    state.snapshots.lock().unwrap().insert(id, snapshot);
    result.map_err(ArchiveError::Io)
}

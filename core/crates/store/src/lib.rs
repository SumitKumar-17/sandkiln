//! Durable history of sandbox lifecycle events — a record that survives
//! a daemon restart, independent of `sandkiln-daemon`'s in-memory
//! `sandboxes`/`snapshots` maps.
//!
//! This is deliberately *not* a replacement for the live in-memory
//! sandbox map, and it cannot bring a stopped daemon's sandboxes back to
//! life: a live `Sandbox` owns a real OS process (a `sandkiln_vmm::Vm`'s
//! `Child` handle, open API/vsock sockets) that has no serializable
//! representation and no re-adoption mechanism if the daemon restarts —
//! today a daemon shutdown of any kind (kill, crash, `systemctl
//! restart`) orphans every live sandbox's Firecracker process
//! unconditionally, confirmed via `scripts/sandkilnd-ctl.sh`'s own
//! best-effort orphan-cleanup step and `SELF_HOSTING.md`'s
//! troubleshooting section. A `Snapshot` (see `sandkiln-daemon::snapshot`)
//! is the only thing that actually resumes into a working sandbox after
//! a restart, and it already has its own durability story (atomic-write
//! JSON + directory-scan reconciliation at startup) — this crate doesn't
//! change or duplicate that.
//!
//! What this *does* solve: `GET /sandboxes` only ever shows what's
//! currently live, and that list is gone the instant the daemon
//! restarts, even for sandboxes that had names/tags worth remembering.
//! `HistoryStore` keeps a durable, queryable record of every sandbox
//! sandkiln has ever created — when, with what tags, and how it ended
//! (destroyed, snapshotted, or orphaned by a daemon restart before
//! either happened) — independent of whether the daemon has restarted
//! since.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Why a sandbox's history record stopped being "still live" —
/// `None`/`NULL` in the `ended_at_unix`/`end_reason` columns means it
/// was live as of the last time anything wrote to this row, which is
/// only trustworthy while the daemon that wrote it is still the one
/// running; see [`HistoryStore::mark_unended_as_orphaned_on_startup`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// Torn down via `?keep=false` or the idle reaper's destroy pass.
    Destroyed,
    /// Stopped via the persistent-by-default path (manual `snapshot()`,
    /// auto-suspend, or a plain `DELETE` with `keep` left at its
    /// default) — `final_snapshot_id` on the record names the result.
    Snapshotted,
    /// Discovered still marked live at daemon startup — the daemon that
    /// created it is gone, and with it any real Firecracker process this
    /// row might have referred to; see the module doc comment for why
    /// that's true unconditionally, not a heuristic.
    OrphanedByRestart,
}

impl EndReason {
    fn as_str(self) -> &'static str {
        match self {
            EndReason::Destroyed => "destroyed",
            EndReason::Snapshotted => "snapshotted",
            EndReason::OrphanedByRestart => "orphaned_by_restart",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryRecord {
    pub id: String,
    pub name: Option<String>,
    pub tags: HashMap<String, String>,
    pub image_id: Option<String>,
    pub created_at_unix: u64,
    pub ended_at_unix: Option<u64>,
    pub end_reason: Option<String>,
    pub final_snapshot_id: Option<String>,
}

/// Options for [`HistoryStore::list`]. All fields default to "no
/// filter" via `Default` — `HistoryFilter::default()` lists everything,
/// newest-created first, up to `limit`.
#[derive(Debug, Default)]
pub struct HistoryFilter {
    /// Only records still live (no `ended_at`) when `Some(true)`, only
    /// ended records when `Some(false)`, everything when `None`.
    pub live_only: Option<bool>,
    /// Maximum rows to return. `None` means the store's own default cap
    /// (see [`DEFAULT_LIST_LIMIT`]) — always bounded, never unlimited,
    /// so a caller can't accidentally pull an unbounded table into memory.
    pub limit: Option<u32>,
}

/// Applied when [`HistoryFilter::limit`] is `None` — a history table
/// only grows, so an unbounded default would get slower and larger
/// forever; callers that actually want more page with `limit` explicitly.
pub const DEFAULT_LIST_LIMIT: u32 = 100;

pub struct HistoryStore {
    conn: Mutex<Connection>,
}

impl HistoryStore {
    /// Opens (creating if missing) the sqlite database at `path` and
    /// ensures its schema exists. `path` should be on the same
    /// persistent storage as `SANDKILN_DRIVES_DIR`/`SANDKILN_IMAGES_DIR`
    /// — see `SANDKILN_HISTORY_DB_PATH` in the daemon's `config.rs`.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                    Some(format!("creating parent directory {}: {e}", parent.display())),
                )
            })?;
        }
        let conn = Connection::open(path)?;
        Self::init_schema(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// An in-memory database — for tests only; nothing durable about it.
    #[doc(hidden)]
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    fn init_schema(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sandbox_history (
                id               TEXT PRIMARY KEY,
                name             TEXT,
                tags_json        TEXT NOT NULL,
                image_id         TEXT,
                created_at_unix  INTEGER NOT NULL,
                ended_at_unix    INTEGER,
                end_reason       TEXT,
                final_snapshot_id TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_sandbox_history_created_at ON sandbox_history(created_at_unix);
             CREATE INDEX IF NOT EXISTS idx_sandbox_history_ended_at ON sandbox_history(ended_at_unix);",
        )
    }

    /// Records a newly-created sandbox. Called once, right after a boot
    /// actually succeeds — a failed boot never reaches this, matching
    /// `GET /sandboxes` only ever listing sandboxes that actually exist.
    pub fn record_created(
        &self,
        id: &str,
        name: Option<&str>,
        tags: &HashMap<String, String>,
        image_id: Option<&str>,
        created_at: SystemTime,
    ) -> rusqlite::Result<()> {
        let tags_json = serde_json::to_string(tags).expect("a String->String map always serializes");
        let created_at_unix = unix_secs(created_at);
        self.conn.lock().unwrap().execute(
            "INSERT INTO sandbox_history (id, name, tags_json, image_id, created_at_unix) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name, tags_json, image_id, created_at_unix],
        )?;
        Ok(())
    }

    /// Records how a sandbox's life ended. `final_snapshot_id` is only
    /// meaningful for [`EndReason::Snapshotted`] — `None` otherwise.
    /// A no-op (not an error) if `id` has no row yet, e.g. a sandbox
    /// created by a daemon build old enough to predate this store —
    /// there's nothing to update, and that's expected, not a bug.
    pub fn record_ended(&self, id: &str, ended_at: SystemTime, reason: EndReason, final_snapshot_id: Option<&str>) -> rusqlite::Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE sandbox_history SET ended_at_unix = ?1, end_reason = ?2, final_snapshot_id = ?3 WHERE id = ?4",
            params![unix_secs(ended_at), reason.as_str(), final_snapshot_id, id],
        )?;
        Ok(())
    }

    /// Marks every still-"live" record as orphaned by a restart — call
    /// once at daemon startup, before accepting requests, the same way
    /// `sandkiln-daemon::snapshot::reconcile` runs before the HTTP
    /// listener binds. See the module doc comment for why this is
    /// unconditionally correct, not a heuristic: nothing survives today
    /// that could still be genuinely live at this point. Returns how
    /// many rows were updated, purely for a log line — a nonzero count
    /// on a fresh install is not itself an error.
    pub fn mark_unended_as_orphaned_on_startup(&self, now: SystemTime) -> rusqlite::Result<usize> {
        self.conn.lock().unwrap().execute(
            "UPDATE sandbox_history SET ended_at_unix = ?1, end_reason = ?2 WHERE ended_at_unix IS NULL",
            params![unix_secs(now), EndReason::OrphanedByRestart.as_str()],
        )
    }

    pub fn get(&self, id: &str) -> rusqlite::Result<Option<HistoryRecord>> {
        self.conn
            .lock()
            .unwrap()
            .query_row("SELECT * FROM sandbox_history WHERE id = ?1", params![id], row_to_record)
            .optional()
    }

    /// Newest-created first. See [`HistoryFilter`] for what can be
    /// filtered; unset fields impose no constraint beyond the default
    /// row limit.
    pub fn list(&self, filter: &HistoryFilter) -> rusqlite::Result<Vec<HistoryRecord>> {
        let limit = filter.limit.unwrap_or(DEFAULT_LIST_LIMIT);
        let conn = self.conn.lock().unwrap();
        let where_clause = match filter.live_only {
            Some(true) => " WHERE ended_at_unix IS NULL",
            Some(false) => " WHERE ended_at_unix IS NOT NULL",
            None => "",
        };
        let sql = format!("SELECT * FROM sandbox_history{where_clause} ORDER BY created_at_unix DESC LIMIT ?1");
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit], row_to_record)?;
        rows.collect()
    }
}

fn row_to_record(row: &rusqlite::Row) -> rusqlite::Result<HistoryRecord> {
    let tags_json: String = row.get("tags_json")?;
    let tags: HashMap<String, String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(HistoryRecord {
        id: row.get("id")?,
        name: row.get("name")?,
        tags,
        image_id: row.get("image_id")?,
        created_at_unix: row.get::<_, i64>("created_at_unix")? as u64,
        ended_at_unix: row.get::<_, Option<i64>>("ended_at_unix")?.map(|v| v as u64),
        end_reason: row.get("end_reason")?,
        final_snapshot_id: row.get("final_snapshot_id")?,
    })
}

fn unix_secs(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tags(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn record_created_then_get_round_trips_every_field() {
        let store = HistoryStore::open_in_memory().unwrap();
        let created_at = SystemTime::now();
        store.record_created("sbx-1", Some("my-name"), &tags(&[("env", "prod")]), Some("img-1"), created_at).unwrap();

        let record = store.get("sbx-1").unwrap().expect("record should exist");
        assert_eq!(record.id, "sbx-1");
        assert_eq!(record.name.as_deref(), Some("my-name"));
        assert_eq!(record.tags.get("env").map(String::as_str), Some("prod"));
        assert_eq!(record.image_id.as_deref(), Some("img-1"));
        assert_eq!(record.created_at_unix, unix_secs(created_at) as u64);
        assert!(record.ended_at_unix.is_none());
        assert!(record.end_reason.is_none());
    }

    #[test]
    fn get_on_a_nonexistent_id_returns_none_not_an_error() {
        let store = HistoryStore::open_in_memory().unwrap();
        assert!(store.get("does-not-exist").unwrap().is_none());
    }

    #[test]
    fn record_ended_sets_reason_and_snapshot_id() {
        let store = HistoryStore::open_in_memory().unwrap();
        store.record_created("sbx-1", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        let ended_at = SystemTime::now();
        store.record_ended("sbx-1", ended_at, EndReason::Snapshotted, Some("snap-1")).unwrap();

        let record = store.get("sbx-1").unwrap().unwrap();
        assert_eq!(record.ended_at_unix, Some(unix_secs(ended_at) as u64));
        assert_eq!(record.end_reason.as_deref(), Some("snapshotted"));
        assert_eq!(record.final_snapshot_id.as_deref(), Some("snap-1"));
    }

    #[test]
    fn record_ended_on_destroy_has_no_snapshot_id() {
        let store = HistoryStore::open_in_memory().unwrap();
        store.record_created("sbx-1", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        store.record_ended("sbx-1", SystemTime::now(), EndReason::Destroyed, None).unwrap();

        let record = store.get("sbx-1").unwrap().unwrap();
        assert_eq!(record.end_reason.as_deref(), Some("destroyed"));
        assert!(record.final_snapshot_id.is_none());
    }

    #[test]
    fn record_ended_on_an_unknown_id_is_a_silent_no_op() {
        let store = HistoryStore::open_in_memory().unwrap();
        // Must not error -- see record_ended's doc comment for why.
        store.record_ended("never-created", SystemTime::now(), EndReason::Destroyed, None).unwrap();
    }

    #[test]
    fn mark_unended_as_orphaned_only_touches_still_live_rows() {
        let store = HistoryStore::open_in_memory().unwrap();
        store.record_created("live-1", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        store.record_created("live-2", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        store.record_created("already-ended", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        store.record_ended("already-ended", SystemTime::now(), EndReason::Destroyed, None).unwrap();

        let updated = store.mark_unended_as_orphaned_on_startup(SystemTime::now()).unwrap();
        assert_eq!(updated, 2);

        assert_eq!(store.get("live-1").unwrap().unwrap().end_reason.as_deref(), Some("orphaned_by_restart"));
        assert_eq!(store.get("live-2").unwrap().unwrap().end_reason.as_deref(), Some("orphaned_by_restart"));
        // Not touched -- was already ended for a real reason before this ran.
        assert_eq!(store.get("already-ended").unwrap().unwrap().end_reason.as_deref(), Some("destroyed"));
    }

    #[test]
    fn list_defaults_to_newest_created_first() {
        let store = HistoryStore::open_in_memory().unwrap();
        let base = SystemTime::now();
        store.record_created("oldest", None, &HashMap::new(), None, base).unwrap();
        store.record_created("newest", None, &HashMap::new(), None, base + Duration::from_secs(10)).unwrap();
        store.record_created("middle", None, &HashMap::new(), None, base + Duration::from_secs(5)).unwrap();

        let records = store.list(&HistoryFilter::default()).unwrap();
        let ids: Vec<&str> = records.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["newest", "middle", "oldest"]);
    }

    #[test]
    fn list_live_only_excludes_ended_records() {
        let store = HistoryStore::open_in_memory().unwrap();
        store.record_created("live", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        store.record_created("ended", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        store.record_ended("ended", SystemTime::now(), EndReason::Destroyed, None).unwrap();

        let records = store.list(&HistoryFilter { live_only: Some(true), limit: None }).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, "live");
    }

    #[test]
    fn list_respects_an_explicit_limit() {
        let store = HistoryStore::open_in_memory().unwrap();
        for i in 0..5 {
            store.record_created(&format!("sbx-{i}"), None, &HashMap::new(), None, SystemTime::now()).unwrap();
        }
        let records = store.list(&HistoryFilter { live_only: None, limit: Some(2) }).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn list_on_an_empty_store_is_an_empty_vec_not_an_error() {
        let store = HistoryStore::open_in_memory().unwrap();
        assert!(store.list(&HistoryFilter::default()).unwrap().is_empty());
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested").join("history.db");
        let store = HistoryStore::open(&db_path).unwrap();
        store.record_created("sbx-1", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        assert!(db_path.exists());
        assert!(store.get("sbx-1").unwrap().is_some());
    }

    #[test]
    fn open_on_an_existing_database_preserves_prior_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("history.db");
        {
            let store = HistoryStore::open(&db_path).unwrap();
            store.record_created("sbx-1", None, &HashMap::new(), None, SystemTime::now()).unwrap();
        }
        let reopened = HistoryStore::open(&db_path).unwrap();
        assert!(reopened.get("sbx-1").unwrap().is_some());
    }
}

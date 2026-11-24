# AGENTS.md: sandkiln-store

Read root `AGENTS.md` first. A durable, queryable history of sandbox lifecycle events: created with what
tags, name and image, and how it ended (destroyed, snapshotted, or orphaned by a daemon restart). Backed
by sqlite (`rusqlite` with the `bundled` feature; no system libsqlite needed).

It cannot bring sandboxes back: a live `Sandbox` owns an OS process with no serializable form, and a
restart orphans every Firecracker process with no re-adoption. Resuming after a restart is the daemon's
`snapshots` module; this crate answers "what existed and how did it end".

## Files

- **`lib.rs`**: `HistoryStore` (`open`, `open_in_memory`, `record_created`, `record_ended`,
  `mark_unended_as_orphaned_on_startup`, `get`, `list`), `HistoryRecord`, `EndReason`, `HistoryFilter`.
  One table, `sandbox_history`, no migrations: extend `init_schema`'s `CREATE TABLE IF NOT EXISTS` with
  nullable columns.

## Daemon wiring

- `AppState::history` opens at startup from `history_db_path`.
- `mark_unended_as_orphaned_on_startup` runs in `main.rs` with `snapshots::reconcile`, before the listener binds.
- `record_created` fires from `create_sandbox_core` after a successful boot only. `record_ended` fires from
  `destroy_sandbox_by_id` (`Destroyed`) and `snapshot_and_stop` (`Snapshotted`, with the snapshot id).
- The HTTP surface is read-only (`GET /sandboxes/history`); every write is a side effect of a lifecycle op.

## Invariants

- One `Mutex<Connection>`, not a pool, for a single-daemon low-QPS store. `rusqlite` over `sqlx` because
  every VM-touching route already runs in `spawn_blocking`.
- `record_ended` on an unknown id is a silent no-op.
- `HistoryFilter::list` has a default row limit on purpose; the table only grows, so page with `limit`.
- `open` sets `journal_mode=WAL` and `synchronous=NORMAL` (about 9.24ms to 37us per write). `open_in_memory`
  is for tests and skips this.

## Testing

`cargo test -p sandkiln-store` uses `open_in_memory`; no KVM needed.

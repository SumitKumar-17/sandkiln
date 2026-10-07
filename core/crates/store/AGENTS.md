# AGENTS.md — sandkiln-store

Read root `AGENTS.md` first.

## What this crate is

A durable, queryable history of sandbox lifecycle events — created with
what tags/name/image, and how it ended (destroyed, snapshotted, or
orphaned by a daemon restart). Backed by sqlite (`rusqlite`, `bundled`
feature, no system `libsqlite3-dev` needed).

**Does not and cannot bring a stopped daemon's sandboxes back to life** —
a live `Sandbox` owns a real OS process with no serializable
representation; a daemon restart orphans every live sandbox's Firecracker
process unconditionally, with no re-adoption mechanism anywhere in this
project (jailer included). `sandkiln-daemon::snapshot` solves "resume
into a working sandbox after a restart"; this crate solves "know what
existed and how it ended" — different problems, not overlapping.

## Files

- **`lib.rs`** — `HistoryStore` (open/open_in_memory/record_created/
  record_ended/mark_unended_as_orphaned_on_startup/get/list),
  `HistoryRecord`, `EndReason`, `HistoryFilter`. One table
  (`sandbox_history`), no migrations — extend `init_schema`'s `CREATE
  TABLE IF NOT EXISTS` with nullable columns instead.

## Wiring into the daemon

- `AppState::history` opens once at startup from
  `SANDKILN_HISTORY_DB_PATH` (default `~/sandkiln-tools/history.db`).
- `mark_unended_as_orphaned_on_startup` runs in `main.rs` alongside
  `snapshot::reconcile`, both before the HTTP listener binds.
- `record_created` fires from `create_sandbox_core` only after a boot
  succeeds (a failed boot never appears in history). `record_ended` fires
  from `destroy_sandbox_by_id` (`Destroyed`) and `snapshot_and_stop`
  (`Snapshotted`, with the resulting snapshot id).
- Read-only via `GET /sandboxes/history` — no write path from the HTTP
  API; every write is a side effect of an existing lifecycle op.

## Non-obvious things

- One `Mutex<Connection>`, not a pool — deliberate for a single-daemon,
  low-QPS store; a pool would add real complexity (busy_timeout tuning)
  for a volume this daemon will never produce.
- `rusqlite` (sync) over `sqlx` (async) on purpose — every VM-touching
  daemon route already runs in `spawn_blocking`, so a sync call fits with
  no new concurrency model.
- `record_ended` on an unknown id is a silent no-op, not an error — a
  sandbox from a daemon build older than this store has nothing to
  update.
- `HistoryFilter::list`'s default row limit is deliberate — the table
  only grows, so page with an explicit `limit` rather than removing it.
- `open` sets `journal_mode=WAL` + `synchronous=NORMAL`, not sqlite's
  defaults — cuts a write from ~9.24ms (measured on a cold create's
  critical path) to ~37µs (isolated A/B, same disk). `open_in_memory`
  (tests only) skips this — no durability concern for `:memory:`.

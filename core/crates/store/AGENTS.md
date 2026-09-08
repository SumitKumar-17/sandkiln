# AGENTS.md — sandkiln-store

Read the root `AGENTS.md` first for project-wide conventions. This file
is scoped to this one crate.

## What this crate is

A durable, queryable history of sandbox lifecycle events — when a
sandbox was created, with what tags/name/image, and how it ended
(destroyed, snapshotted, or orphaned by a daemon restart before either
happened). Backed by sqlite (`rusqlite`, `bundled` feature — compiled
from source, no system `libsqlite3-dev` needed).

**Read `src/lib.rs`'s module doc comment before touching anything here**
— it explains the one thing this crate deliberately does *not* do: bring
a stopped daemon's sandboxes back to life. A live `Sandbox` owns a real
OS process with no serializable representation; today a daemon restart
of any kind orphans every live sandbox's Firecracker process
unconditionally (confirmed via `scripts/sandkilnd-ctl.sh`'s own
best-effort orphan-cleanup and `SELF_HOSTING.md`'s troubleshooting
section) — there is no re-adoption mechanism anywhere in this project to
build toward, jailer included. `sandkiln-daemon::snapshot` already
solves "actually resume into a working sandbox after a restart"; this
crate solves a different problem — "know what existed and how it ended,
even after a restart" — and the two aren't in tension or overlapping.

## Files

- `lib.rs` — `HistoryStore` (open/open_in_memory, record_created,
  record_ended, mark_unended_as_orphaned_on_startup, get, list),
  `HistoryRecord`, `EndReason`, `HistoryFilter`. One table
  (`sandbox_history`), no migrations framework — if the schema needs to
  change, extend `init_schema`'s `CREATE TABLE IF NOT EXISTS` additively
  (new nullable columns) rather than introducing a migration system for
  what's still a single-table, personal-project-scale store.

## Wiring into the daemon

- `sandkiln-daemon::state::AppState` holds a `history: HistoryStore`
  field, opened once at startup from `SANDKILN_HISTORY_DB_PATH` (default
  `~/sandkiln-tools/history.db`, matching `SANDKILN_DRIVES_DIR`/
  `SANDKILN_IMAGES_DIR`'s own default-path convention in `config.rs`).
- `mark_unended_as_orphaned_on_startup` is called once in `main.rs`,
  right alongside `snapshot::reconcile` — both run before the HTTP
  listener binds, for the same reason: reconcile durable state against
  reality before any request can observe a stale view of it.
- `record_created` is called from `routes_sandbox::create_sandbox_core`
  right after a boot actually succeeds (a failed boot never reaches it —
  `GET /sandboxes/history` should never show something that never
  really existed). `record_ended` is called from both
  `routes_sandbox::destroy_sandbox_by_id` (`EndReason::Destroyed`) and
  `routes_snapshot::snapshot_and_stop` (`EndReason::Snapshotted`, with
  the resulting snapshot id).
- Exposed read-only via `GET /sandboxes/history` (`routes_sandbox.rs`) —
  there's no write path from the HTTP API; every write happens as a
  side effect of an existing lifecycle operation, not a new caller
  action.

## Non-obvious things

- **The whole store is one `Mutex<Connection>`, not a connection pool.**
  Deliberate — this is a self-hosted, single-daemon-process, low-QPS
  store (see `SELF_HOSTING.md`'s framing), not a multi-tenant service;
  a pool would add real complexity (sqlite's own concurrent-writer
  semantics, `busy_timeout` tuning) for a workload this daemon's actual
  request volume will never produce. If that assumption ever stops
  holding, that's a deliberate, evidenced decision to revisit — not
  something to "fix" preemptively.
- **`rusqlite` (sync) was chosen over `sqlx` (async) on purpose.** Every
  VM-touching route in `sandkiln-daemon` already runs inside
  `tokio::task::spawn_blocking` (see that crate's own `AGENTS.md`, "The
  sandbox map lock is a plain `std::sync::Mutex`...") — a sync sqlite
  call fits directly into that existing pattern with no new concurrency
  model. Don't introduce `sqlx`/an async pool here; it would fight the
  daemon's established architecture for no benefit.
- **`record_ended` on an id with no row is a silent no-op, not an
  error.** A sandbox created by a daemon build that predates this store
  has nothing to update — that's expected, not a bug, and callers in
  `sandkiln-daemon` shouldn't have to special-case it.
- **`HistoryFilter::list`'s default row limit exists on purpose** — this
  table only grows (nothing ever deletes a row), so an unbounded default
  query would get slower forever. Page with an explicit `limit` rather
  than raising or removing the default.

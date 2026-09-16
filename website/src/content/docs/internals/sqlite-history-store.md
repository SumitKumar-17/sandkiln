---
title: "SQLite: the history store"
description: "What sandkiln's durable sandbox history is built on, and the pragma choice that cut a write's latency by ~51x."
---

`GET /sandboxes` only ever shows what's live right now. That list is gone the instant the daemon restarts, even for a sandbox that had a name and tags worth remembering. Something needs to survive the daemon dying without needing the daemon to still be running to read it back: a single-file SQLite database.

## What it is

SQLite is a relational database that lives as one ordinary file on disk, with the whole database engine linked directly into the process that uses it. There's no separate server to run, connect to, or keep alive; a client library talks to it through function calls, not a network protocol. That makes it the wrong choice for a system serving many concurrent writers across machines, and close to the right choice for exactly the opposite case: one process, one file, state that needs to survive a restart without operational overhead.

Two words matter for the write-latency story below:

- **fsync** is the system call that tells the operating system "actually flush this to durable storage before you return control to me." Without it, a write can sit in an in-memory OS page cache and vanish on a power loss, even though the program that wrote it already moved on as if it succeeded.
- **Journal** is SQLite's mechanism for surviving a crash mid-write. Before changing the real database file, it records enough information elsewhere to undo or replay the change if the process dies partway through. Different journal modes trade off how that recording happens, and how many fsyncs it costs.

## Why sandkiln uses it here

A live `Sandbox` in the daemon's in-memory map owns a real Firecracker OS process: an open API socket, an open vsock connection, a live PID. None of that is serializable, and there's no code anywhere in this project to re-adopt an orphaned Firecracker process after a daemon restart (`sandkiln-store`'s own module doc comment is explicit about this: it does not and cannot bring a stopped daemon's sandboxes back to life). A `Snapshot`, by contrast, already has a real durability story of its own: atomic-write JSON and directory reconciliation at startup, covered in [Snapshot, resume, and fork](./snapshot-resume-fork/).

The history store solves a narrower, different problem. Not "make this sandbox usable again," just "remember that it existed, with what tags, and how it ended," independent of whether the daemon has restarted since. SQLite was picked over `sqlx`'s async pool on purpose: every VM-touching route in the daemon already runs inside `tokio::task::spawn_blocking`, and `rusqlite`'s synchronous calls fit that pattern directly, without introducing a second concurrency model for one small store. The whole thing is one `Mutex<Connection>`, not a connection pool. This is a self-hosted, single-daemon-process, low-QPS store, not a multi-tenant service; a pool would add real complexity (SQLite's own concurrent-writer semantics, `busy_timeout` tuning) for a request volume this daemon will never produce.

## Key terms

| Term | Meaning here |
| --- | --- |
| `journal_mode` | How SQLite protects against a crash mid-write. `DELETE` (the default) writes a separate rollback-journal file per transaction and deletes it on commit. `WAL` (write-ahead log) instead appends committed changes to one shared log file, checkpointed back into the main database periodically. |
| `synchronous` | How aggressively SQLite calls fsync. `FULL` (the default) fsyncs on every transaction commit, and under `DELETE` mode specifically fsyncs *twice*: once for the journal, once for the main database file. `NORMAL` fsyncs less often; combined with `WAL` mode it's still crash-safe against an application crash, and safe against everything except an OS/power loss hitting the narrow window before the next periodic checkpoint. |
| `pragma` | A SQLite-specific `PRAGMA <name> = <value>;` statement that configures the connection or database itself, rather than reading or writing table data. |
| `sandbox_history` | The one table this store has. No migrations framework, since a single-table, personal-project-scale store doesn't need one; a schema change extends `init_schema`'s `CREATE TABLE IF NOT EXISTS` with new nullable columns instead. |

## How it works in sandkiln

Everything lives in `core/crates/store/src/lib.rs`, exposed as `HistoryStore`. `HistoryStore::open(path)` opens (or creates) the database file and calls `configure_for_write_latency`, which sets exactly two pragmas:

```rust
fn configure_for_write_latency(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    Ok(())
}
```

`HistoryStore::open_in_memory` (used only in tests) skips this. WAL isn't meaningful for a `:memory:` database, and there's no durability concern for a store that persists nothing anyway.

Three write paths touch the table, all wired in from `sandkiln-daemon`, none of them exposed as a write endpoint on the HTTP API. Every write happens as a side effect of an existing lifecycle operation:

- `record_created`, called from `routes_sandbox::create_sandbox_core` right after a boot actually succeeds. A failed boot never reaches it, so `GET /sandboxes/history` can never show something that never really existed.
- `record_ended`, called from `routes_sandbox::destroy_sandbox_by_id` (`EndReason::Destroyed`) and `routes_snapshot::snapshot_and_stop` (`EndReason::Snapshotted`, carrying the resulting snapshot id). A no-op, not an error, if the id has no row: a sandbox created by a daemon build old enough to predate this store has nothing to update, and that's expected.
- `mark_unended_as_orphaned_on_startup`, called once in `main.rs` before the HTTP listener binds, right alongside snapshot reconciliation. Any row still marked "live" at startup was written by a daemon process that's now gone, so it's marked `orphaned_by_restart` before any request can observe a stale view of it.

`record_created`'s own caller treats a write failure as best-effort. The sandbox has already actually booted by the time this runs, so failing the whole request over a history-write error would waste a real, running sandbox for no benefit. That existing tolerance is exactly what made loosening `synchronous` to `NORMAL` a reasonable choice rather than a compromise: the write was never allowed to block correctness, only to cost time.

## See it in action

A cold create, followed by a destroy, followed by reading it back from history. Three real calls against a running daemon:

```
$ curl -s -X POST http://127.0.0.1:7777/sandboxes \
    -H 'content-type: application/json' \
    -d '{"tags":{"purpose":"docs-example"}}'
{"id":"58186bbf-1f0a-462f-bd5d-9e6c116219a8"}

$ curl -s -X DELETE 'http://127.0.0.1:7777/sandboxes/58186bbf-1f0a-462f-bd5d-9e6c116219a8?keep=false'
# HTTP/1.1 204 No Content

$ curl -s 'http://127.0.0.1:7777/sandboxes/history?limit=1'
{"history":[{"id":"58186bbf-1f0a-462f-bd5d-9e6c116219a8","name":null,"tags":{"purpose":"docs-example"},"image_id":null,"created_at_unix":1789536161,"ended_at_unix":1789536170,"end_reason":"destroyed","final_snapshot_id":null}]}
```

The row survives even though the sandbox itself is gone from `GET /sandboxes`. That's the entire point of a separate durable store.

### The pragma change, measured

Before this session, `HistoryStore::open` used SQLite's own defaults (`journal_mode=DELETE`, `synchronous=FULL`): fsync twice per write, once for the journal and once for the database file. That measured at **~9.24ms on a real cold create's critical path** (5.5% of the total create time). An isolated A/B on the same disk, 20 sequential `record_created` calls, default settings against the fix, nothing else running, measured:

```
default settings:  mean=1897.8µs  min=1360µs  max=6956µs
WAL + NORMAL:       mean=37.1µs   min=33µs    max=60µs
```

A ~51x cut, confirmed by a dedicated test (`open_sets_wal_and_synchronous_normal_for_write_latency`) that asserts the pragmas actually took effect, not just that opening the store doesn't error:

```rust
let journal_mode: String = conn.pragma_query_value(None, "journal_mode", |row| row.get(0)).unwrap();
assert_eq!(journal_mode, "wal");
let synchronous: i64 = conn.pragma_query_value(None, "synchronous", |row| row.get(0)).unwrap();
assert_eq!(synchronous, 1); // 1 = NORMAL (0 OFF, 2 FULL, 3 EXTRA)
```

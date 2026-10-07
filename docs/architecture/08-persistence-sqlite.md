# Durable sandbox history (sqlite)

## What it's for

The daemon's live sandbox map is in-memory — gone on restart. `sandkiln-store`
(`HistoryStore`) is a small, separate sqlite-backed crate that durably records
every sandbox that ever existed and how it ended, specifically so that
information survives a daemon restart the live map never could.

## The write-latency fix — a real, measured algorithmic change

Default sqlite settings (`journal_mode=DELETE`, `synchronous=FULL`) fsync
**twice per write** — once for the rollback journal, once for the main
database file. `HistoryStore::open` (not `open_in_memory`, which needs none of
this) now sets:

- **`journal_mode=WAL`** — write-ahead logging instead of a rollback journal;
  writers append to a separate WAL file instead of touching the main database
  file synchronously.
- **`synchronous=NORMAL`** — only safe specifically *because* WAL mode is on
  (NORMAL under the old rollback-journal mode risks corruption on a crash; under
  WAL it only risks losing the most recent transactions, not corrupting the
  database) — and reasonable here because this store's writes were already
  best-effort by design (`record_created`'s caller only warns on failure, never
  blocks the sandbox operation itself on a history-write failure).
- **Measured, isolated A/B on the same disk**: **1897.8µs → 37.1µs** mean write
  latency — roughly **51x**. A dedicated scratch benchmark, not a guess.

## Status

Done, unit-tested (`open_sets_wal_and_synchronous_normal_for_write_latency`
asserts the actual pragma values), shipped. See
[`website/src/content/docs/internals/sqlite-history-store.md`](../../website/src/content/docs/internals/sqlite-history-store.md)
for the full timing methodology.

# Snapshot, resume, and fork

## The mechanism

Firecracker itself supports pausing a running VM and writing its full memory +
device state to disk (`mem.bin` + `state.snap`), then later restoring a *new*
Firecracker process from those same files. sandkiln's snapshot is that
mechanism plus the bookkeeping needed to make it a real product feature:
tracking which rootfs, network config, and resource limits go with it, and what
happens when two different sandboxes try to resume from the same one.

## Key structures and rules

- **`source_snapshot_id` vs `parent_snapshot_id`.** Two different relationships,
  kept as two different fields on `Snapshot`, precisely because conflating them
  produced a real bug earlier in this project: `source_snapshot_id` is what you
  resumed *from* to reach a given live sandbox; `parent_snapshot_id` is the
  snapshot-to-snapshot lineage edge once that sandbox is snapshotted again.
- **`forked_into` — a one-live-descendant lock.** A snapshot can be resumed into
  at most one *live* sandbox at a time from `fork`; a second concurrent fork
  attempt gets a real `409 Conflict`, not silent corruption or two VMs sharing
  one rootfs.
- **Fork vs. resume**: resume reuses the snapshot's identity going forward;
  fork clones it into a genuinely independent new sandbox (its own rootfs clone,
  its own network identity) while the original snapshot stays untouched and
  re-forkable.
- **Snapshot lineage** is a parent-pointer DAG, queryable in both directions
  (ancestors and descendants of a given snapshot) — not just a flat list.
- **Retired snapshots / time-travel restore.** A `RetiredSnapshot`
  (`snapshot_history.rs`) is a checkpoint that's been consumed by a resume/fork
  but kept around as inert, restorable data — restoring it again doesn't consume
  it, unlike a normal snapshot resume. This is what makes "go back to exactly
  this point, repeatedly" possible instead of one-shot.
- **A snapshot points at paths, not values.** The rootfs referenced by a
  snapshot is a path on disk, not bytes embedded in the snapshot metadata — the
  real rootfs-corruption bug this project hit and fixed came from exactly this
  distinction being handled wrong once (two live sandboxes' clones pointed at
  the same underlying file after a resume).
- **Egress policy re-applies** across resume/fork/time-travel-restore (tied to
  the network lease going live again, not stored *in* the snapshot itself —
  see [03](03-networking-and-egress.md)).

## Status

Done, live-verified, durable across a daemon restart (`sandkiln-store`, see
[08](08-persistence-sqlite.md)). SDK/CLI: snapshot/resume/fork are exposed; snapshot
lineage and time-travel restore are daemon-HTTP-API-only so far (see
`ROADMAP.md`'s "Persistence and snapshotting" section for the exact current
surface). See [`website/src/content/docs/internals/snapshot-resume-fork.md`](../../website/src/content/docs/internals/snapshot-resume-fork.md)
for the full rootfs-bug postmortem and a real captured `409` from a double-fork
attempt.

# Warm pools and snapshot lineage

## Pre-warmed snapshot pools

Restoring an already-initialized VM instead of booting one is the standard
Firecracker cold-start fix, and sandkiln already had the mechanism. Where the
detail lives (read these, they are not repeated here):

- The concept, claim flow and a worked example:
  [pools internals page](../../docs-site/internals/pre-warmed-pools.mdx) and
  [startup latency](../../docs-site/architecture/startup-latency.mdx).
- The measurements and what they do and do not say (claim latency 70-200ms
  vs cold 160-200ms, 25-100x to first exec, the 1-in-3 to 2-in-3 resume
  failure rate, the 10.3s vs 5.4s `force_stop` fallback, `max_count` queueing
  with 30s then 503, `PoolClaimGuard` and the bounded 3-attempt retry):
  [engineering post 21](../../docs-site/engineering/21-opt-pool-prewarming.md).
- The MMDS re-init after resume:
  [MMDS internals page](../../docs-site/internals/mmds.mdx).

Facts kept here because those pages do not carry them:
- `POST /pools` takes `{id, image_id?, vcpu_count?, mem_size_mib?,
  warm_count, max_count?}`; `pool_replenisher` ticks every 2s. Pool identity
  is image plus resolved vcpu/mem. `warm_count: 0` is valid (inert pool). SDK
  and CLI: `Pool.create/list/delete` (JS/TS and Python, sync and async),
  `kiln pool create|ls|rm`.
- Pool configuration is in memory only. After a daemon restart a caller
  re-POSTs, and a warm snapshot with no pool left to claim or clean it can be
  orphaned (known edge).
- The ~180ms gap between a ~32ms boot and the measured 211ms create was never
  in boot or resume (both about 25-32ms) but in per-create setup, since
  profiled as almost entirely the rootfs prep (the lease is about 4ms). A
  pool's real value is pre-doing that setup ahead of a request.
- The resume-failure rate was measured across clean, isolated, sequential
  single-claim runs at 1 in 3 to 2 in 3. `08-snapshots.sh`'s single
  resume-then-exec check still passes every run because one resume is too few
  attempts to hit it; a pool resumes far more often. Same family as the open
  finding 20 ([findings](../findings.md)). Fallbacks showed 0 caller-visible
  failures across every stress run, and the daemon log confirms the fallback
  firing at that rate.
- The max_count bug was visible as `GET /pools` `claimed` reading 0 right
  after a health-check failure, found by live testing, not review. Three
  consecutive bad resumes still fall through unattributed: an accepted rare
  edge.
- The MMDS failure message after resume is "MMDS data store is not
  initialized" on a bare `PATCH /mmds`; `Vm::update_metadata` redoes the full
  `PUT /mmds/config` plus `PUT /mmds`.

## Snapshot lineage

`Snapshot.parent_snapshot_id` is the snapshot the new snapshot's source
sandbox was resumed or forked from; `None` marks a lineage root (a
cold-booted source). `GET /snapshots` exposes it on every `SnapshotSummary` and accepts
`?parent_snapshot_id=<id>` (what came from this) beside
`?source_sandbox_id=` (what did this come from). Unlike `?source_sandbox_id=`, the parent filter
can match more than one snapshot (a snapshot can be forked, that fork
snapshotted and torn down, then forked again into a sibling line). Together they walk a full
tree in either direction. Persisted in `meta.json`.

Bug caught live: reusing `Sandbox::source_snapshot_id` for the pointer
dead-ended every resumed sandbox's lineage, because that field is set only
on forks and deliberately `None` on resume (so a resumed sandbox stays
snapshottable under the `ForkedFrom` guard). It compiled and passed its own
tests; a live resume-then-snapshot chain queried by `?parent_snapshot_id=`
came back empty. Fixed with a separate `Sandbox::parent_snapshot_id` set on
both resume and fork, `None` only for a cold boot. 10 new integration checks (267/267 overall at the time).

Deliberately narrow: this is a parent pointer, not a durable ancestry
record. A deleted intermediate snapshot breaks the visible chain. A durable
ancestry table would be an extension, not a bug fix. Daemon API only, no
SDK or CLI surface yet.

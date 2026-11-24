# Warm pools and snapshot lineage

## Pre-warmed snapshot pools

Restoring an already-initialized VM instead of booting one is the standard
Firecracker cold-start fix, and sandkiln already had the mechanism.

- `POST /pools` takes `{id, image_id?, vcpu_count?, mem_size_mib?,
  warm_count, max_count?}`. A background replenisher (`pool_replenisher`,
  ticking every 2s) keeps `warm_count` resumable snapshots ready.
- A plain `POST /sandboxes` matching a pool's `image_id` and resolved
  vcpu/mem claims a warm snapshot transparently; there is no separate
  "create from pool" call.
- SDK and CLI: `Pool.create/list/delete` (JS/TS and Python, sync and
  async), `kiln pool create|ls|rm`.
- Pool identity is image plus resolved vcpu/mem. `warm_count: 0` is valid
  (inert pool). A request with `drives` or a custom `rate_limit` never
  matches a pool, since both are baked in at boot; it cold-creates.
- Pool configuration is in memory only. After a daemon restart a caller
  re-POSTs, and a warm snapshot with no pool left to claim or clean it can
  be orphaned (known edge).

### Measured win

A clean claim measured about 70-200ms vs a cold create's 160-200ms at
`create()` return, a modest win. Counted through to first exec the gap is
25-100x: a cold create's first exec pays 420-460ms for the guest agent to
start, a claim pays 4-18ms. See [benchmarks](../benchmarks.md).

### Resume failures and the health check

Resuming a snapshot sometimes yields a guest kernel that panics early
(earlier console logs showed a divide-by-zero trap in the console driver)
and Firecracker exits. In clean sequential runs this hit roughly 1 in 3 to
2 in 3 resumes at the time; it was invisible until pools resumed often.
It is the same family as the open finding 20 (see
[findings](../findings.md)).

Handled, not ignored: every claim runs `exec true` after resume. A failing
claim tears the sandbox down with `Vm::force_stop` (skipping `stop()`'s
optimistic pre-kill `sync`; without this a fallback paid two ~5s retry
timeouts, about 10.3s total, instead of one, about 5.4s) and falls back to a
normal cold create, so a caller never receives a dead sandbox. Stress runs
showed 0 caller-visible failures.

### MMDS after resume

Firecracker restore does not preserve MMDS's initialized state, so
`PATCH /mmds` fails with "MMDS data store is not initialized".
`Vm::update_metadata` redoes `PUT /mmds/config` plus `PUT /mmds`, so a
claimed sandbox's guest sees its real identity, not the warm placeholder's.

### `max_count` ceiling and queueing

`max_count` caps live instances (warm plus claimed). A `POST /sandboxes`
matching a pool at its ceiling with nothing warm queues (a
`tokio::sync::Notify` per pool, woken on replenish or on any live instance
stopping) for up to 30s, then returns 503. `GET /pools` reports
`max_count` and `claimed`. Verified: a `max_count: 1` pool blocks a second
claim, wakes it the instant the first stops, and 503s after the full 30s.

Bug found live: a failed warm claim fell through to an unattributed cold
create, so a bounded pool could exceed its ceiling during a bad run of
resumes (visible as `claimed` reading 0 right after a health-check
failure). Fixed with a bounded 3-attempt retry: the failed claim's slot is
released (`PoolClaimGuard::drop`) and the loop re-resolves the pool so the
cold-create fallback is attributed. Three consecutive bad resumes still
fall through unattributed, an accepted rare edge.

## Snapshot lineage

`Snapshot.parent_snapshot_id` is the snapshot the new snapshot's source
sandbox was resumed or forked from; `None` marks a lineage root (a
cold-booted source). `GET /snapshots` exposes it and accepts
`?parent_snapshot_id=<id>` (what came from this) beside
`?source_sandbox_id=` (what did this come from). Together they walk a full
tree in either direction. Persisted in `meta.json`.

Bug caught live: reusing `Sandbox::source_snapshot_id` for the pointer
dead-ended every resumed sandbox's lineage, because that field is set only
on forks and deliberately `None` on resume (so a resumed sandbox stays
snapshottable under the `ForkedFrom` guard). It compiled and passed its own
tests; a live resume-then-snapshot chain queried by `?parent_snapshot_id=`
came back empty. Fixed with a separate `Sandbox::parent_snapshot_id` set on
both resume and fork, `None` only for a cold boot. 10 integration checks.

Deliberately narrow: this is a parent pointer, not a durable ancestry
record. A deleted intermediate snapshot breaks the visible chain. A durable
ancestry table would be an extension, not a bug fix. Daemon API only, no
SDK or CLI surface yet.

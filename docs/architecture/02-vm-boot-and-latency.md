# VM boot, startup latency, pre-warmed pools, idle lifecycle

## Boot mechanism

`sandkiln-vmm::vm::boot` drives the real Firecracker binary through its own HTTP
API over a Unix socket — `PUT /boot-source`, `/drives`, `/machine-config`,
`/vsock`, then `PUT /actions` with `InstanceStart`. No shelling out to a Firecracker
CLI; every call is a typed Rust HTTP client (`firecracker_api.rs`) against
Firecracker's own OpenAPI-described socket.

## Measured latency, not estimated

- Raw KVM boot: **~11ms** (was ~32ms — a fixed 20ms sleep waiting on the vsock
  socket was found and removed; see `ROADMAP.md`'s Benchmarking section for the
  investigation).
- Full `POST /sandboxes` cold create: **~144ms** average.
- **The real bottleneck, found by actually profiling instead of guessing**: the
  rootfs clone (`cp --reflink=auto` per sandbox, `routes_sandbox.rs`'s rootfs-clone
  helper) is **74%** of that time, not the network lease this project originally
  suspected (~4ms). `--reflink=auto` uses copy-on-write when the filesystem
  supports it (btrfs/XFS with reflink); the dev box runs plain ext4, where it
  silently degrades to a full byte copy — the concrete, honest remaining gap,
  documented rather than hand-waved.
- Network lease (tap device claim, IP assignment) and the rootfs clone run
  **concurrently** (`spawn_blocking_in_current_span`, not sequentially) precisely
  because profiling showed they were independent and sequential ordering was
  wasting wall-clock for no reason.

## Pre-warmed pools

A pool keeps `warm_count` ready-to-resume snapshots around per
(image, resource-config) so a matching `POST /sandboxes` can **resume** one
instead of paying full cold-create cost — `pool.rs`, `pool_replenisher.rs`.

- **Two independent knobs**: `warm_count` (latency — how many warm snapshots to
  keep on hand) and `max_count` (concurrency — total live instances this pool's
  profile may ever have at once, `None` = unbounded).
- **Producer/consumer with a bounded wait, not an unbounded queue.** A request
  that can't get a warm snapshot and is at `max_count` **queues** via
  `Pool::notify` (a `tokio::sync::Notify`), waiting up to `pool_claim::POOL_QUEUE_TIMEOUT`
  (30s) before returning a real `503` — never hangs a caller indefinitely.
- **Scoped honestly**: a request with `drives` or a custom `rate_limit` never
  matches a pool at all (both are baked into a VM at boot time; a warm snapshot
  was booted with neither) — falls through to cold-create rather than silently
  ignoring the request's own settings.
- Pool *configuration* lives only in memory (`AppState::pools`) — not durable
  across a daemon restart, unlike snapshots themselves.

## Idle lifecycle (tiered)

`idle_reaper.rs` runs one background tick that does three independent things,
each gated by its own optional timeout:

1. **Auto-suspend** (`SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`) — pause + snapshot an
   idle sandbox, keeping it resumable.
2. **Destroy** (`SANDKILN_IDLE_TIMEOUT_SECS`) — VM killed, network lease released,
   rootfs deleted, for a sandbox idle even longer.
3. **Archive** (`SANDKILN_ARCHIVE_TIMEOUT_SECS`) — move an already-held snapshot's
   files off the live `snapshots_root()` onto `Config::archive_dir` once it's old
   enough, independent of whether either sandbox-side timeout is even configured
   (it's about a snapshot's own age).

The reaper task and the pool replenisher task both spawn **unconditionally** at
daemon startup, even with nothing configured — a no-op tick is cheap, and this
avoids a class of bug where a feature silently doesn't run because a spawn was
gated on a check someone forgot to update.

## Status

Done, live-verified. See `ROADMAP.md`'s "Benchmarking" and "Persistence and
snapshotting" sections for the full numeric history.

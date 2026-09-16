---
title: "Internals: pre-warmed pools"
description: Pre-warming as a general latency-hiding technique, and how sandkiln's replenisher keeps a pool topped up in the background.
---

For the full measured latency finding this feature is built around, see [Startup latency & the pre-warmed pool](../../architecture/startup-latency/). This page is about the mechanism itself.

## What it is

**Pre-warming** is a general technique for hiding startup latency: instead of doing expensive initialization work at the moment it's needed, do it *ahead of time*, speculatively, and keep a small supply of already-initialized instances ready to hand out. The tradeoff is always the same, some idle capacity sits around doing nothing most of the time, in exchange for the next request not having to pay the initialization cost at all. It's the same idea behind a database connection pool, a thread pool, or a web server keeping a few pre-forked worker processes on standby.

## Why sandkiln uses it here

A cold sandbox's first real exec (not just the boot succeeding, the guest agent actually being reachable and ready) measured 420-460ms end to end on this project's own dev box. A resumed sandbox, whose guest agent is already running in the snapshotted memory the moment it resumes, measured 4-18ms to the same milestone, a 25-100x difference. `snapshot`/`resume`/`fork` already existed as the underlying mechanism (see [Snapshots, resume, and fork](../../concepts/snapshots/)); pools are what turns "a caller can manually resume a snapshot they made earlier" into "the daemon keeps ready snapshots warm proactively, so a plain create just happens to be fast."

## Key terms

- **`warm_count`**: a pool's target number of ready-to-resume snapshots. A target, not a hard cap, a claim past what's currently warm just cold-creates instead of failing.
- **Replenishment**: the background process of keeping a pool topped up toward `warm_count`. sandkiln's replenisher wakes on a fixed interval (`CHECK_INTERVAL`, currently 2 seconds) and boots, pauses, and snapshots a fresh instance for any pool that's below target, tagged `sandkiln.pool` so it's identifiable in `GET /sandboxes` mid-replenish.
- **`max_count`**: an optional hard ceiling on the *total* live instances (warm plus claimed) for a pool's profile. A claim that would exceed it queues instead of cold-creating unbounded, and returns a real `503` after 30 seconds if nothing frees a slot in time.
- **Post-resume health check**: every claim from a pool runs a real `exec true` against the resumed sandbox before handing it to the caller. This exists because resuming a snapshot has a real, non-rare Firecracker/KVM failure mode (a guest-kernel panic on restore, measured at roughly 1-in-3 to 2-in-3 resumes on this dev box, see the startup-latency page linked above), so a failed health check falls back to a normal cold create transparently rather than ever handing back a broken sandbox id.

## How it works in sandkiln

A plain `POST /sandboxes`/`Sandbox.create()` that requests nothing a warm snapshot can't already provide, no `drives`, no custom `rate_limit`, both baked into Firecracker's boot-time configuration, which a snapshot has already fixed, matches against any configured pool whose image and resource profile (vCPU count, memory) agree. If a warm snapshot is ready, the daemon resumes it, runs the health check, and, on success, overwrites the resumed sandbox's placeholder name/tags/MMDS content with the caller's real ones (`claim_from_pool`, which is also what makes MMDS correctly reflect the *claimer's* identity rather than the pool's own internal bookkeeping, see [Internals: MMDS](../mmds/)) before returning it. There's no separate "claim from pool" API call, a caller never explicitly opts in or out; it's entirely transparent based on what the request does and doesn't ask for.

## See it in action

```
$ curl -s -X POST http://127.0.0.1:7777/pools \
    -H 'content-type: application/json' \
    -d '{"id":"docs-demo-pool","warm_count":1}'

{"id":"docs-demo-pool","image_id":null,"vcpu_count":2,"mem_size_mib":512,"warm_count":1,"max_count":null,"warm_ready":0,"claimed":0}
```

`warm_ready` starts at `0`, the pool was just configured and the replenisher hasn't had a chance to act yet. Checking again about 12 seconds later, after the replenisher's own 2-second check interval has had time to boot, pause, and snapshot a fresh instance:

```
$ curl -s http://127.0.0.1:7777/pools

{"pools":[{"id":"docs-demo-pool","image_id":null,"vcpu_count":2,"mem_size_mib":512,"warm_count":1,"max_count":null,"warm_ready":1,"claimed":0}]}
```

`warm_ready` is now `1`, a real snapshot sitting ready to resume. A plain `POST /sandboxes` matching this pool's profile (2 vCPU, 512MiB, the daemon's own defaults, no drives, no rate limit) would resume it instead of cold-booting, and `warm_ready` would drop back to `0` until the replenisher catches up again.

---
title: Startup latency & the pre-warmed pool
description: Why the next latency win isn't a faster boot.
---

sandkiln's cold boot measures 32.3–33.1ms (`criterion`, `core/crates/vmm/benches/vm_lifecycle.rs::bench_cold_boot`) and Firecracker's own published numbers document sub-second, even sub-125ms, boot times industry-wide — sandkiln's own number is consistent with that. But "boot fast" and "start instantly" aren't the same claim, and the biggest lever for the second one isn't a faster boot at all. See the project's Performance page for the full current benchmark numbers this builds on.

## The documented industry technique

Production Firecracker users solve cold-start latency by not booting from scratch at all: they snapshot a VM that's already past initialization — kernel loaded, runtime warmed, dependencies imported — and restore *that* on the next request instead of repeating the boot-and-initialize sequence every time. The saving isn't in the boot path getting faster; it's in skipping most of the path entirely.

sandkiln already has the exact mechanism this needs:

- `snapshot`/`resume`/`fork` (see [Snapshots, resume, and fork](/docs/concepts/snapshots/)) save a running sandbox's full memory and disk state and boot a new one directly from it.
- Auto-suspend (`SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`, see [Auto-suspend idle sandboxes](/docs/guides/auto-suspend/)) already turns an idle sandbox into a resumable snapshot automatically.

The primitive is not the gap.

## The actual gap: reactive, not proactive

Every snapshot sandkiln produces today exists because something already happened — a caller explicitly called `snapshot()`, or a sandbox sat idle long enough for auto-suspend to catch it (`core/crates/daemon/src/idle_reaper.rs`). There's no path where a snapshot is prepared *ahead of* a request that hasn't arrived yet, the way a pre-warmed pool would keep one ready. Pre-warming turns "resume is faster than cold boot" into "the caller never pays the cold-boot cost at all," because a ready-to-resume snapshot is already sitting there when the request lands.

## Concrete next step: a pre-warmed snapshot pool

Keep a small pool of ready-to-resume snapshots per image/config, populated ahead of demand rather than only after an idle timeout. A `get-or-create` or create-from-image call that finds a matching pooled snapshot resumes it instead of cold-booting; the pool backfills in the background.

This needs an actual resume-latency number to justify the pool size and refill rate — which doesn't exist yet either. There's no `criterion` benchmark for resume (only `bench_cold_boot`/`bench_exec_roundtrip` exist today); the only number on record is a manually-verified ~39ms for a single resume during snapshot/resume development, not a repeatable measurement. Both — the benchmark and the pool — are the next real work here, not a vaguer "optimize boot."

## What Firecracker itself already buys, for free

Two things worth naming, since they're already load-bearing and not something sandkiln had to build:

- **Minimal device model.** Firecracker emulates only five devices (virtio-net, virtio-block, virtio-vsock, serial console, keyboard controller). A smaller device surface is both a smaller attack surface and less to initialize at boot — sandkiln doesn't add anything beyond Firecracker's own default set.
- **Jailer's defense-in-depth.** The jailer (see [Privilege model](/docs/architecture/privilege-model/)) does privileged setup once — cgroups, chroot, seccomp — then drops privileges and execs into an unprivileged Firecracker process, rather than running the VM itself with any elevated rights. sandkiln's own jailer support (opt-in, `SANDKILN_JAILER_ENABLED`) follows this same shape.

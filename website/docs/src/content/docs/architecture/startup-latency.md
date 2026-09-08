---
title: Startup latency & the pre-warmed pool
description: The pre-warmed pool is shipped — what it actually buys, what it found, and what's still the real next lever.
---

sandkiln's cold boot measures 32.3–33.1ms (`criterion`, `core/crates/vmm/benches/vm_lifecycle.rs::bench_cold_boot`) and Firecracker's own published numbers document sub-second, even sub-125ms, boot times industry-wide — sandkiln's own number is consistent with that. But "boot fast" and "start instantly" aren't the same claim. See the project's Performance page for the full current benchmark numbers this builds on.

## The documented industry technique, now shipped proactively

Production Firecracker users solve cold-start latency by not booting from scratch at all: they snapshot a VM that's already past initialization — kernel loaded, runtime warmed, dependencies imported — and restore *that* on the next request instead of repeating the boot-and-initialize sequence every time. sandkiln had the primitive for a while (`snapshot`/`resume`/`fork`, see [Snapshots, resume, and fork](../../concepts/snapshots/); auto-suspend, see [Auto-suspend idle sandboxes](../../guides/auto-suspend/)) before it had the proactive half.

**Pre-warmed pools close that gap.** `POST /pools` configures a pool (`{id, image_id?, vcpu_count?, mem_size_mib?, warm_count}`); a background replenisher keeps `warm_count` resumable snapshots ready per pool; a plain `POST /sandboxes`/`Sandbox.create()` — no `drives`, no custom `rate_limit`, since both are baked into a VM's boot-time state and a warm snapshot has neither — matching a pool's image/resources resumes a ready snapshot automatically instead of cold-booting. Entirely transparent: there's no separate "create from pool" call. `Pool.create/list/delete` in the JS/TS and Python SDKs, `kiln pool create|ls|rm` in the CLI.

## What was measured

- **A clean claim** (the resumed snapshot passes its post-resume health check, below) lands at roughly **70–200ms**, against a cold create's own **~160–200ms** on the same box — a real, modest win, consistent with resume's own criterion number (~25.8ms, only ~19% faster than cold boot's ~31.9ms) sitting well under a full create's overhead.
- That clean case is **not the reliable common case on this dev box today** — see the failure-rate finding below, which is the more significant result this work actually produced.

## The real finding: resuming a snapshot has a non-rare failure mode

Building a pool means resuming far more often, in quick succession, than any prior manual test ever had — and that surfaced a real Firecracker/KVM behavior: a resumed guest kernel can panic early in boot (confirmed via Firecracker's own captured console log — an early-boot divide-by-zero trap in the console driver, restored CPU/timer state interacting badly with timing-sensitive init code). The whole Firecracker process exits shortly after.

Measured directly and repeatedly, across clean, isolated, sequential single-claim test runs with no other load on the box: the failure rate ranged from roughly **1 in 3 to 2 in 3** resumes — not a rare edge case. The project's own snapshot/resume integration check still passes on every run, because a single resume attempt isn't enough to reliably hit a fraction-of-attempts failure like this one.

The root cause is genuinely open — plausibly TSC/clock-source drift between snapshot-time and restore-time, but that's an informed guess, not a confirmed diagnosis. Whether this is specific to this dev box's kernel/KVM/CPU combination or a broader Firecracker snapshot/restore characteristic is unconfirmed. See the [Engineering notebook](../engineering-notebook/) for the full investigation, including the two follow-on fixes it produced (a post-resume health check with automatic cold-create fallback, and `Vm::force_stop` to avoid paying two independent timeouts recovering from one bad resume).

**Handled, not just documented**: every claim runs a real post-resume health check (`exec true`) before being handed to a caller. A failed check tears the sandbox down and falls back to a normal cold create — a caller never receives a dead sandbox id. Confirmed via repeated stress tests showing zero caller-visible failures across every run, with the daemon's own logs confirming the fallback path firing at the rate the direct measurements predicted.

## What's still scoped out of this first slice

- **No `max_count` ceiling or queueing yet.** A pool's warm buffer has a target size but no cap on total concurrent live instances — a claim past what's currently warm just cold-creates, unbounded, same as if no pool existed at all.
- **Pool configuration is in-memory only**, not durable across a daemon restart the way snapshot records are — a caller has to re-`POST /pools` afterward, and a restart can orphan an already-warm snapshot with no pool left to claim or clean it up.

## What's next: the rootfs copy, not boot or resume

Snapshot-take (~322ms) and resume (~25.8ms) are both already small — resume was never the bottleneck once measured directly. The ~180ms gap between a ~32ms boot and a full cold create's ~211ms mean lives in the surrounding per-create setup, dominated by the rootfs copy: this dev box runs ext4, which has no copy-on-write, so `cp --reflink=auto` silently falls back to an ordinary copy. A CoW-capable filesystem (XFS, Btrfs) would make that same call an instant clone with no code change; a device-mapper/thin-provisioning layer would close the same gap a different way if the host filesystem can't change. This is the concrete next lever for every create the pool above doesn't already serve from a warm snapshot — see the Performance page for a few further ideas, explicitly flagged as brainstorming rather than shipped or verified work.

## What Firecracker itself already buys, for free

Two things worth naming, since they're already load-bearing and not something sandkiln had to build:

- **Minimal device model.** Firecracker emulates only five devices (virtio-net, virtio-block, virtio-vsock, serial console, keyboard controller). A smaller device surface is both a smaller attack surface and less to initialize at boot — sandkiln doesn't add anything beyond Firecracker's own default set.
- **Jailer's defense-in-depth.** The jailer (see [Privilege model](../privilege-model/)) does privileged setup once — cgroups, chroot, seccomp — then drops privileges and execs into an unprivileged Firecracker process, rather than running the VM itself with any elevated rights. sandkiln's own jailer support (opt-in, `SANDKILN_JAILER_ENABLED`) follows this same shape, though it's not yet proven on real hardware — see the [Engineering notebook](../engineering-notebook/).

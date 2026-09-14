---
title: Startup latency & the pre-warmed pool
description: The pre-warmed pool is shipped — what it actually buys, what it found, and what's still the real next lever.
---

sandkiln's cold boot measures 10.5–10.9ms (`criterion`, `core/crates/vmm/benches/vm_lifecycle.rs::bench_cold_boot` — was 32.3–33.1ms before a fixed 20ms socket-wait sleep was found and fixed, see below) and Firecracker's own published numbers document sub-second, even sub-125ms, boot times industry-wide — sandkiln's own number is consistent with that. But "boot fast" and "start instantly" aren't the same claim. See the project's Performance page for the full current benchmark numbers this builds on.

## The documented industry technique, now shipped proactively

Production Firecracker users solve cold-start latency by not booting from scratch at all: they snapshot a VM that's already past initialization — kernel loaded, runtime warmed, dependencies imported — and restore *that* on the next request instead of repeating the boot-and-initialize sequence every time. sandkiln had the primitive for a while (`snapshot`/`resume`/`fork`, see [Snapshots, resume, and fork](../../concepts/snapshots/); auto-suspend, see [Auto-suspend idle sandboxes](../../guides/auto-suspend/)) before it had the proactive half.

**Pre-warmed pools close that gap.** `POST /pools` configures a pool (`{id, image_id?, vcpu_count?, mem_size_mib?, warm_count}`); a background replenisher keeps `warm_count` resumable snapshots ready per pool; a plain `POST /sandboxes`/`Sandbox.create()` — no `drives`, no custom `rate_limit`, since both are baked into a VM's boot-time state and a warm snapshot has neither — matching a pool's image/resources resumes a ready snapshot automatically instead of cold-booting. Entirely transparent: there's no separate "create from pool" call. `Pool.create/list/delete` in the JS/TS and Python SDKs, `kiln pool create|ls|rm` in the CLI.

## What was measured

- **A clean claim** (the resumed snapshot passes its post-resume health check, below) lands at roughly **70–200ms**, against a cold create's own **~160–200ms** on the same box, comparing how fast each one's `create()`/claim call itself returns — a real, modest win on its own.
- **That comparison undersells it — a later investigation found the real gap is 25-100x, not 2x.** `create()` returning 200 means Firecracker's `InstanceStart` succeeded, not that the guest agent is listening yet: a cold sandbox's actual first `exec` measures **~420-460ms** end to end (every exec after the first on the same sandbox measures ~3-5ms, confirming it's a one-time tax), while a resumed sandbox — agent already running in the snapshotted memory — measures **~4-18ms** to first exec. Counted through to "the sandbox can actually run something," not just "the call returned," a pre-warmed pool's real win is far larger than either number above suggests on its own. See the [Engineering notebook](../engineering-notebook/) for the full story of how this was found (auditing a retry-loop bug fix that turned out to matter far less than what it led to discovering).
- That clean case is **not the reliable common case on this dev box today** — see the failure-rate finding below, which is the more significant result this work actually produced.

## The real finding: resuming a snapshot has a non-rare failure mode

Building a pool means resuming far more often, in quick succession, than any prior manual test ever had — and that surfaced a real Firecracker/KVM behavior: a resumed guest kernel can panic early in boot (confirmed via Firecracker's own captured console log — an early-boot divide-by-zero trap in the console driver, restored CPU/timer state interacting badly with timing-sensitive init code). The whole Firecracker process exits shortly after.

Measured directly and repeatedly, across clean, isolated, sequential single-claim test runs with no other load on the box: the failure rate ranged from roughly **1 in 3 to 2 in 3** resumes — not a rare edge case. The project's own snapshot/resume integration check still passes on every run, because a single resume attempt isn't enough to reliably hit a fraction-of-attempts failure like this one.

The root cause is genuinely open — plausibly TSC/clock-source drift between snapshot-time and restore-time, but that's an informed guess, not a confirmed diagnosis. Whether this is specific to this dev box's kernel/KVM/CPU combination or a broader Firecracker snapshot/restore characteristic is unconfirmed. See the [Engineering notebook](../engineering-notebook/) for the full investigation, including the two follow-on fixes it produced (a post-resume health check with automatic cold-create fallback, and `Vm::force_stop` to avoid paying two independent timeouts recovering from one bad resume).

**Handled, not just documented**: every claim runs a real post-resume health check (`exec true`) before being handed to a caller. A failed check tears the sandbox down and falls back to a normal cold create — a caller never receives a dead sandbox id. Confirmed via repeated stress tests showing zero caller-visible failures across every run, with the daemon's own logs confirming the fallback path firing at the rate the direct measurements predicted.

## `max_count`: an optional ceiling with queueing

A pool's `warm_count` is a target size, not a hard cap on how many live instances of its profile can exist — by default, a claim past what's currently warm just cold-creates, unbounded. An optional `max_count` caps the total (warm + claimed) instead: a `POST /sandboxes` that would exceed it queues, waking the instant a slot frees up (a claimed instance stops, or a new warm snapshot finishes replenishing) rather than polling, and returns a real `503` after 30 seconds if nothing frees up in time — never silently exceeding the ceiling, never hanging a caller's request forever.

## What's still scoped out

- **Pool configuration is in-memory only**, not durable across a daemon restart the way snapshot records are — a caller has to re-`POST /pools` afterward, and a restart can orphan an already-warm snapshot with no pool left to claim or clean it up.

## What's next: measured wrong once, then measured completely

Snapshot-take (~322ms) and resume (~7ms, after the fix below) are both already small — resume was never the bottleneck once measured directly. This section previously named the rootfs copy as the remaining bottleneck, then measured a real XFS loopback filesystem and concluded the copy was already hidden behind the concurrent network lease, so a CoW filesystem or device-mapper layer wouldn't help. **That conclusion was wrong** — caught by actually profiling every phase rather than re-reading the reasoning.

A full per-phase pass instrumented `create_sandbox_cold` and `Vm::boot` and ran 20 isolated cold creates, one at a time, nothing else on the box. It accounted for 167.33 of 167.65ms measured:

| Phase | Share of a cold create |
| --- | --- |
| rootfs clone (`cp --reflink=auto`) | 74.0% |
| `Vm::boot` total | 20.3% |
| — wait for the API socket (fixed below; was 12.0%) | 0.5% |
| — `InstanceStart` | 7.1% |
| — configuration PUTs, all 7 combined | 1.0% |
| history-store write | 5.5% |
| network lease (fully concurrent with the clone) | ~0.1% |

The network lease — the thing blamed above — measured **~4.36ms**, not ~130ms. The rootfs clone measured **124.09ms, 74% of the whole create**. The two were swapped: the clone was never hidden behind the lease, because the lease was never big enough to hide anything behind.

**What that same pass found and fixed**: `Vm::boot`'s wait for Firecracker's freshly spawned API socket used a fixed `sleep(20ms)` before ever trying to connect — measured **20.11ms on every single one of the 20 boots** (min 20.04, max 20.18), the signature of a sleep nobody needed to wait that long for. Replaced with a real connect-retry on a 200µs→5ms backoff, which also closes a genuine race the file-existence check had (the socket file appears at `bind()`, a moment before `listen()`). Re-measured: boot **34.00ms → 11.22/11.44ms**, cold create **167.65ms → 144.59/143.56ms** — a real **~14%** cut to every cold create, and to snapshot resume too, since it shares the same code path. Verified with the full integration suite (300/300, snapshot/resume/time-travel included) and `cargo clippy`/`cargo test` clean.

**The correction, stated plainly**: the CoW filesystem test was real and its own numbers stand — only the explanation attached to them was wrong. The leading replacement, **not yet re-verified**: the per-sandbox clone always lands in `std::env::temp_dir()` regardless of where the base image lives, so the earlier test's clone could never have actually reflinked even though the base image itself sat on XFS. A device-mapper layer would have hit the identical wall for the identical reason and remains not planned. `scripts/preflight-check.sh` still reports whether rootfs storage sits on a CoW-capable filesystem — a real disk-space win regardless, just not a confirmed latency one yet.

Also unexamined: the synchronous history-store write sitting on the critical path at 9.24ms (5.5% of a create), for a write whose own code already treats it as best-effort — a plausible easy win, not yet attempted. See the [Engineering notebook](../engineering-notebook/) for the full story, including this project's own mistake reaching the wrong conclusion the first time.

## What Firecracker itself already buys, for free

Two things worth naming, since they're already load-bearing and not something sandkiln had to build:

- **Minimal device model.** Firecracker emulates only five devices (virtio-net, virtio-block, virtio-vsock, serial console, keyboard controller). A smaller device surface is both a smaller attack surface and less to initialize at boot — sandkiln doesn't add anything beyond Firecracker's own default set.
- **Jailer's defense-in-depth.** The jailer (see [Privilege model](../privilege-model/)) does privileged setup once — cgroups, chroot, seccomp — then drops privileges and execs into an unprivileged Firecracker process, rather than running the VM itself with any elevated rights. sandkiln's own jailer support (opt-in, `SANDKILN_JAILER_ENABLED`) follows this same shape, though it's not yet proven on real hardware — see the [Engineering notebook](../engineering-notebook/).

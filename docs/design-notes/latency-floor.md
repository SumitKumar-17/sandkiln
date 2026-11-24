# Latency floor: what is achievable, where sandkiln is, what closes the gap

Question: what is the lowest start latency a microVM sandbox can reach, why
is sandkiln not there, and what exactly would close the gap? Written
2026-11-24. External claims carry a URL; anything not confirmed from a
primary source is marked **unverified**. Our numbers are from
[benchmarks](../benchmarks.md) and
[startup latency](../../docs-site/architecture/startup-latency.mdx): one
shared box, Firecracker v1.16.1, guest kernel 5.10.223 with a systemd
rootfs, directional only.

## Headline

1. There are two different floors. A **cold boot** has to run a guest
   kernel and an init; the best documented figures put the VMM-to-init
   window at roughly 125ms or less for a minimal kernel and about 20ms of that
   is a static network interface. A **restored** microVM skips all of it and
   is bounded by the memory-file mapping and the first guest page faults:
   single-digit milliseconds.
2. sandkiln's cold path (214.5ms to first command) is not limited by the
   host. Host work is about 16ms. About 125ms is systemd userspace (not Firecracker's 125ms init bound, a different quantity) and about 70ms is
   kernel plus agent start (derived, see below). The gap to a well-built cold
   path is **guest userspace**, not Firecracker.
3. sandkiln's warm path (resume 4ms, first exec 8.4ms) is already close to
   the documented restore floor. What is missing is making it the default
   user-visible `create`, and per-clone state fixups (entropy, clock) that a
   restore-based create needs for correctness.
4. Snapshot take (310-727ms) is a different cost: synchronous write of all
   guest memory. It is off the critical path when a pool absorbs it.

## What the primary sources say

### Firecracker's own numbers and how they are measured

| Claim | Figure | Measured how | Source |
| --- | --- | --- | --- |
| Boot time | `<= 125ms` | From receiving `InstanceStart` to the guest's `/sbin/init` starting; serial console disabled, minimal kernel and rootfs; CI on bare-metal instances | [SPECIFICATION.md](https://github.com/firecracker-microvm/firecracker/blob/main/SPECIFICATION.md) |
| Boot, serial, end to end | "Less than 125ms" to application code; 150 microVMs/s/host | Paper abstract | [NSDI 2020 paper](https://www.usenix.org/system/files/nsdi20-paper-agache.pdf) |
| Boot methodology | Time from VMM process fork to guest init; kernel 4.14.94, 1 vCPU, 256MB, minimal init that writes an IO port; 500 serial samples | Section on boot times | same |
| Concurrency | 1000 microVMs, 50 at a time: p99 146ms (pre-configured Firecracker) vs 158ms (Cloud Hypervisor); 153ms at 100 concurrent | same | same |
| Console | Disabling serial console logging saves "up to 70ms" | same | same |
| Static network | A statically configured interface adds "around 20ms" (Firecracker, Cloud Hypervisor), about 35ms (QEMU) | same | same |
| Kernel config | A stock distro kernel took "an additional 900ms", partly legacy-device probe timeouts and unneeded drivers; compressed-kernel decompression adds about 40ms | same | same |
| Snapshot restore | No latency figure published; "very fast snapshot loading", take is slower because it writes memory synchronously | Snapshot docs | [snapshot-support.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/snapshot-support.md) |

Caveat that matters for comparisons: Firecracker's 125ms ends at `init`
starting, not at an application answering. Our 214.5ms ends at a real command
succeeding, so the two are not the same interval. Our own `InstanceStart`
phase (about 13ms) is not "boot time" either, it is the host starting vCPUs.

The same paper notes the VMM can be made faster by moving config into one
API call and doing memory allocation and kernel load during configuration
(listed as investigated, not shipped in the paper).

### Cold boot: what dominates inside the guest

| Cost | Evidence | Source |
| --- | --- | --- |
| Serial console output | Each byte is a trapped UART write; 70ms in the NSDI setup, about 90ms here | NSDI paper above; [engineering post 28](../../website/src/content/engineering/28-opt-guest-boot-time.md) |
| Pre-created `/dev/loopN` | 124ms `loop_init` here (our measurement, fixed by `loop.max_loop=1`) | post 28 |
| Driver probing and legacy device timeouts | 900ms on a distro kernel; near zero on a minimal config | NSDI paper |
| Static IP via `ip=` | 18.7ms `ip_auto_config` here, about 20ms in NSDI | NSDI paper; post 28 |
| Init system and device units | about 125ms systemd userspace here (not Firecracker's init bound), `dev-vda.device` 107ms (our measurement) | post 28 |
| Method | `initcall_debug` plus `printk.time=1`, graph with the kernel's boot-graph script; `quiet` removes console cost | [Bootlin boot-time slides](https://bootlin.com/doc/legacy/optimizations/embedded_linux_optimizations.pdf) |
| Specialized kernel | A Linux specialized by configuration reports a 23ms hello-world boot in the authors' slides; 71ms without kernel-mode-linux. Unikernel-style, not our workload | [Lupine slides](https://2020.eurosys.org/wp-content/uploads/2020/04/slides/159_kuo_slides.pdf), [paper](https://people.cs.vt.edu/djwillia/papers/eurosys20-lupine.pdf); figures from search snippets, **unverified** |
| Direct boot of an uncompressed kernel | PVH direct boot roughly halved time to kernel start (95ms to 48ms) in a 2018 QEMU RFC | [qemu-devel RFC](https://lists.nongnu.org/archive/html/qemu-devel/2018-12/msg00957.html); old, **unverified** for current versions |

Firecracker already loads an uncompressed `vmlinux` directly, so the
decompression and bootloader costs do not apply to us.

### Restore: file-backed, userfaultfd, page cache, huge pages, diffs

| Topic | What is documented or published | Source |
| --- | --- | --- |
| File backend | Memory file is a private on-demand mapping, kernel handles faults; the file must stay for the VM's lifetime and be treated as immutable | [snapshot-support.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/snapshot-support.md) |
| Uffd backend | A separate handler process serves faults via `UFFDIO_COPY`; no benchmark numbers published; a crashed handler hangs the VM on its next fault | [handling-page-faults](https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/handling-page-faults-on-snapshot-resume.md) |
| Huge pages | `2M` hugetlbfs requires the Uffd backend; dirty tracking "negates most of the benefits"; THP effect "may be limited" with Uffd | snapshot-support.md |
| Diff snapshots | Developer preview; needs `track_dirty_pages` (KVM dirty log, runtime cost, not saved in the snapshot, must be re-set on load); diffs are merged onto a base with `snapshot-editor` | snapshot-support.md |
| Durability knob | `sync_snapshot_files` defaults to true (fsync before return); false keeps data in the page cache and same-host reads still see it, but a host crash can lose it | snapshot-support.md |
| Working set | On-demand restore causes thousands of serialized page faults; a function's working set is 3-39% of guest memory (9% average); recording and prefetching it (REAP) cut cold start 3.7x on average | [REAP, ASPLOS 2021](https://arxiv.org/pdf/2101.09355) |
| Page-cache effect | Disk-resident snapshots are up to 3.5x slower than page-cache-resident ones; page-cache minor faults average 3.7us and 90% finish under 8us | [FaaSnap, EuroSys 2022](https://www.sysnet.ucsd.edu/~voelker/pubs/faasnap-eurosys22.pdf) |
| Load call cost | A file-backed load call returns in single-digit milliseconds in another project's measurements (3.5-4.1ms); our `resume` is 3.7-4.0ms | [issue #504, another project](https://github.com/kvcache-ai/AgentENV/issues/504); our benchmarks |
| Process-level fork | A restore/fork primitive for gVisor-class sandboxes reports sub-millisecond to about 2ms starts, against about 142ms baseline; a different isolation model, **unverified** figures | [Catalyzer, ASPLOS 2020](https://ipads.se.sjtu.edu.cn/pub/projects/catalyzer) |

Reading: with the memory file in the page cache, file-backed restore is the
right backend for a single host. Uffd earns its complexity when memory is
remote, deduplicated or needs prefetch ordering; huge pages force it and
conflict with dirty tracking. Nothing here argues for uffd on this box.
I did not find a trustworthy figure for a memory file on tmpfs;
**unverified**, and the page cache already gives the same effect once warm.

### Cheap clones and per-VM unique state

| State | Documented guidance | Source |
| --- | --- | --- |
| Entropy | Firecracker always enables VMGenID; guests on Linux >= 5.18 reseed the kernel CSPRNG on resume. A race window remains between vCPU resume and reseed, so reseeding explicitly (`RNDADDENTROPY`, `RNDRESEEDCRNG`) is still recommended. Userspace-cached randomness and ids are NOT fixed | [random-for-clones.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/random-for-clones.md) |
| Boot id, seeds | Clones read the same `/proc/sys/kernel/random/boot_id`; delete systemd random-seed files before snapshotting | same |
| Clock | Wall clock continues from snapshot time, the guest must update it; `clock_realtime` on load (x86_64, host >= 5.16, TSC host clocksource) advances it | snapshot-support.md |
| Network | Connectivity is not preserved across resume; identical guest IPs behind per-clone namespaces plus NAT is the documented example, which the page itself calls not performant or secure; `network_overrides` re-points the TAP on load | [network-for-clones.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/network-for-clones.md) |
| Never | Resuming the same snapshot state more than once is considered insecure unless clones are made unique | snapshot-support.md |
| Rootfs | Copy-on-write file clone (reflink) is what we already use; same technique as any CoW rootfs | [startup latency](../../docs-site/architecture/startup-latency.mdx) |

### Host-side floor

Our own measurements are the evidence here; I found no published figure for
a bare KVM `KVM_CREATE_VM` plus vCPU creation, **unverified**. Locally:
process spawn 0.34ms, API socket ready 0.67ms, seven configuration PUTs
1.61ms, reflink clone 0.08ms, tap lease about 3-4ms (concurrent with the
clone, 0.17ms on the critical path), history write 37us. `InstanceStart`
(about 13ms) is where Firecracker creates the VM and vCPUs and loads the
kernel; the NSDI paper proposes moving memory allocation and kernel load into
the configuration phase to shrink exactly this window. Host work is about
16ms in total and is not the bottleneck.

## Gap analysis, cold path

Our column is measured unless marked derived. "Best known" is the best
figure I can support for this class of hardware; where I could not source a
number it is an estimate from the cited components and says so.

| Stage | Ours | Best known achievable | Why the gap | Lever |
| --- | --- | --- | --- | --- |
| Host spawn | 0.34ms | under 1ms | none | none; done |
| VMM config (PUTs, socket wait) | 2.3ms (1.61 + 0.67) | about 1ms with one combined call | seven round trips | cosmetic; not worth a custom VMM path |
| Rootfs clone and lease | 0.08ms, tap about 3.5ms concurrent | same | none | done (reflink, pre-created tap pool) |
| `InstanceStart` | about 13ms | about 5-13ms | kernel load and VM/vCPU creation inside the call | measure first (strace timing), few levers |
| Create call total | 16-18ms | 10-15ms | the rows above | none worth the effort |
| Kernel boot | about 80-107ms (measured: guest `dmesg` on 2026-11-24 shows `systemd[1]` starting at 0.107s guest uptime, and the init-mode agent starts at 80-110ms; includes 18.7ms `ip_auto_config`). The earlier derived 70ms (214.5 - 16 - 125) was too low | 40-60ms estimate, **unverified**: the spec bound is 125ms to init with serial off and a minimal kernel | CI kernel config (5.10.223, broad drivers); static `ip=` costs about 20ms | trimmed kernel config; configure the interface from the agent instead of `ip=` (moves cost, may overlap) |
| Init and userspace | about 125ms systemd (`dev-vda.device` 107ms, `fcnet.service` 45ms) | under 10ms with the agent as PID 1, **unverified** estimate | a general init waiting on udev device units and running network setup for a guest that needs one process | custom minimal PID 1 that mounts `/proc /sys /dev`, starts the agent, configures the NIC |
| Agent ready | polled with 1-20ms backoff, bounded 5s | event-driven | a poll interval adds up to 20ms of overshoot per retry; invisible next to systemd, visible once init is fast | guest connects out to the host over vsock when ready (host listens on the vsock UDS port), create blocks until that signal |
| First exec | 245us warm connection; first one folds in connect | about 0.3-1ms | none | none |
| **Total to first command** | **214.5ms** | **cold goal below** | guest userspace | see ranking |

The earlier 70ms kernel figure was derived from separate boots and did not sum
exactly (post 28 says so for its 197ms/125ms split too). Re-measure with
`initcall_debug` after the init change, because removing systemd also
removes work that currently overlaps with kernel tail.

## Gap analysis, warm path

| Stage | Ours | Best known | Why the gap | Lever |
| --- | --- | --- | --- | --- |
| Pool claim to resumed VM | 4ms resume (3.7-4.0) | about 3.5-4ms | none; file-backed load is cheap | none |
| First exec after resume | 8.4ms | under 5ms | guest page faults on first touch, plus the daemon's `exec true` health check before returning (pool path) | keep pool memory files in page cache; trim health check to one cheap round trip; later, working-set prefetch |
| Metadata refresh | MMDS re-init after resume, retried | not on the floor | MMDS comes back uninitialized | none soon |
| User-visible `create` | pool claim measured 70-200ms in September, **before** the guest-boot work; no current figure | about 15-20ms | stale number; pool is opt-in per profile, not the default | re-measure; make the pool the default path |
| Snapshot take, for the pool refill | 310-727ms | dirty pages only | full-memory write plus fsync, synchronously | `sync_snapshot_files` review, diff snapshots (see below); not user-visible if the pool absorbs it |
| Clone uniqueness | **not done** in the guest agent (no reseed, no clock set found in `core/crates/guest-agent` or `vmm`) | per the Firecracker guidance above | restore-based create hands out identical clones | reseed and set clock on claim; guest kernel 5.10 has no VMGenID reseed (needs >= 5.18), so explicit reseed is mandatory, not optional |

Resumed first exec at 8.4ms versus the 4ms load call tells us about 4ms is
spent after the VM runs again. FaaSnap's 3.7us minor-fault figure says the
page cache is not the problem when warm; the first exec touches a few
hundred pages at most. Whether the remaining time is faults, the vsock
reconnect or the health check is **not measured**; measure before acting.

## Levers, ranked by expected gain per effort

| # | Lever | Path | Expected gain | Effort | Status |
| --- | --- | --- | --- | --- | --- |
| 1 | Custom minimal init as PID 1 running the agent, replacing systemd | cold | about 100-120ms of the 125ms (estimate) | medium: init in the guest-agent crate or a tiny sibling, image build script, NIC setup moved in | **built, opt-in per image, measured**: median to first command 232ms to 147ms (n=30 x 2 against 4 default runs), goal 110ms not met; remaining time is mostly the kernel. Not the default yet (see below). `images/inject-agent.sh --init` |
| 2 | Pool as the default create path, with a refill policy, so `create` is a resume | both | cold users see 214ms to about 15-20ms; the largest user-visible win | medium: default profile, sizing, eviction, honest metrics for misses | partial: pools exist, opt-in per profile |
| 3 | Per-clone fixups on claim: entropy reseed, clock set, hostname/id, boot id | warm | no latency gain, a correctness prerequisite for lever 2 | small-medium: agent RPC plus daemon call in the claim flow | done: `Request::ResumeFixup`, sent after every resume, claim and fork (finding 28) |
| 4 | Agent-initiated vsock readiness signal; `create` can block until ready | cold | about 0-20ms, and makes "200 means ready" true | small | not started: needs a daemon and vmm change (host listens on the vsock UDS, create waits) which was outside the init change; with init mode the poll starts at 1ms and the agent starts at 80-110ms of guest uptime, so the headroom is probably small, but it is unmeasured |
| 5 | Trimmed guest kernel config (drop unused drivers/features, keep virtio, vsock, ext4, fuse as needed) | cold | about 10-30ms estimate, **unverified** | medium: build and maintain a kernel; `build-guest-kernel.sh` exists for FUSE only | not started |
| 6 | Read-only rootfs plus overlay (tmpfs upper) | cold, warm | little latency; smaller snapshot and diff, safe sharing | medium; affects drives and persistence semantics | not started |
| 7 | Keep pool memory files hot in the page cache; fadvise/touch after take | warm | removes the disk-resident penalty (FaaSnap: up to 3.5x on cold-cache restores) | small | not started |
| 8 | Diff snapshots for repeated take of one sandbox | snapshot-take | proportional to dirtied pages, potentially large for idle sandboxes; **unmeasured** | medium-large: dirty tracking has runtime cost, merge chain, file lifetime rules | not started; dev-preview upstream |
| 9 | Review `sync_snapshot_files=false` for pool entries (replaceable, so durability is cheap to give up) | snapshot-take | removes the fsync share of take; **unmeasured** | small | not started |
| 10 | Working-set recording and prefetch (REAP/FaaSnap style) | warm | 3.7x on cold-start page-fault cost in the papers; only matters if first-exec time is fault-bound | large; needs uffd or record/prefetch machinery | not started, gate on measurement |
| 11 | Uffd backend, huge pages | warm | none on one host with a warm cache; huge pages block dirty tracking | large | rejected for now (see above) |

Already done and counted in the 214.5ms: reflink clone (124ms to 0.08ms),
socket-wait backoff (20ms to 0.67ms), history WAL (1.9ms to 37us),
`quiet loop.max_loop=1` (413ms to 214.5ms), concurrent clone and lease,
pre-created tap pool, bounded vsock I/O, file-backed snapshot load.

## Lever 1, result and what remains before it is the default

Measured and recorded in [benchmarks](../benchmarks.md#init-mode-image-the-agent-as-pid-1-2026-11-24):
`create_to_first_command` median 232ms to 147ms. The guest's own clock says
init plus agent start is now a small share (the agent process starts at
80-110ms of guest uptime, first command answered by 100-150ms); the rest is
the kernel, which is lever 5 and the `ip=` interface bring-up. Milestone A's
110ms target is therefore **not met**, and the estimate of "under 10ms of
userspace" holds only if the kernel stage is ignored.

The init-mode image is the recommended path for production latency today, as
an opt-in image (`inject-agent.sh --init` on a copy, registered under a name),
and the full integration suite passes on it (353/353). Before it can become
the default image: (1) run the remote-mounts topic with a FUSE kernel and an
object store on it; (2) decide what to do about images that depend on systemd
units (`build-universal-image.sh` images: services such as cron, ssh or a
user's own units do not start, `systemctl poweroff` has no manager); (3) make
`build-universal-image.sh` and `scripts/dev.sh inject-agent` take `--init` so
the image is reproducible rather than a hand-made copy; (4) apply the same to
the pool and the `kiln doctor` path and re-run the parallel benchmark on an
idle box; (5) look at guest entropy at cold boot (an init-mode guest has no
systemd seeding the pool; the kernel's own jitter entropy covered the tests,
but crypto-heavy first commands are unmeasured).

## Goals and how to measure them

Both goals are read from `examples/benchmark-suite` (owned by another
agent; this note only names its metrics) using its JSON output and `compare`
mode, on an otherwise idle box, and are cross-checked against
`scripts/bench-report.sh`. Each goal states the metric, the percentile and the
run shape.

| Goal | Metric (benchmark-suite) | Target | Basis |
| --- | --- | --- | --- |
| Cold, milestone A (custom init) | `create_to_first_command`, p50 over 20 sequential | at most 110ms | 214.5 minus about 105ms of systemd, estimate |
| Cold, milestone B (kernel trim, readiness signal) | same | at most 80ms | A minus about 30ms; floor about 65ms (16ms host, about 45ms kernel, a few ms init) is an estimate |
| Warm, default `create` | `create_to_first_command` with a matching pool, p50 | at most 20ms, p99 at most 50ms | 4ms resume + 8.4ms first exec + HTTP; the p99 bound covers the resume-failure fallback, which falls back to a cold create and will exceed it, so report its rate separately |
| Warm, resume | `resume_call` p50 / `resume_first_command` p50 | at most 5ms / at most 8ms | already met (4ms / 8.4ms, first one marginal) |
| Parallel | `parallel_create_to_first_command_c8` p95 | at most 2x the sequential p50 | catches lease and clone contention |
| Snapshot take | `snapshot_take` p50 | track, no target until lever 8 or 9 is measured | load-sensitive |

Rules for claiming a win: run before and after back to back, at least 20
samples, record both in `docs/benchmarks.md`, and report an unexplained
regression in any other metric. Do not quote the estimate columns above as
results.

## Order of work

1. Lever 3 first, because lever 2 changes semantics (clones) and must not
   ship without it, and the 5.10 guest has no automatic reseed.
2. Lever 1 and lever 4 together (a custom init is the natural owner of the
   readiness signal); re-profile with `initcall_debug` afterwards to see
   what the kernel stage really costs.
3. Lever 2, then re-measure the pool win with the current boot defaults
   (the September numbers predate them).
4. Levers 7 and 9 as cheap experiments; lever 5 only if the post-init
   profile shows the kernel stage dominating; lever 8 only after
   `snapshot_take` is split into pause, memory write and fsync.

## What I could not verify

- The exact per-percentile values of the NSDI boot figures (the paper's
  charts are not in extractable text), only the quoted summary numbers.
- Any published Firecracker snapshot-restore latency from the project itself:
  its docs give none. The 3.5-4.1ms figure is another project's.
- Lupine's 23ms and the PVH 95ms to 48ms figures (search snippets and an old
  RFC) and Catalyzer's exact start latencies.
- A current memory-on-tmpfs result, and KVM VM/vCPU creation cost in isolation.
- The kernel/agent split of our 214.5ms (derived, not measured in one boot).

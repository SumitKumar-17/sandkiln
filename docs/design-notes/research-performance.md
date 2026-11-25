# Research: performance and density, what to do next

Question: which performance and density techniques are worth building next
for sandkiln, how much would each move, and how would we know. Written
2026-11-25 from the repository (benchmarks, the latency and snapshot notes,
`core/crates/vmm/src`, the daemon's pool and network code) and from primary
sources read that day. Our numbers are directional, one shared i9-13900K box,
Firecracker v1.16.1, host kernel 6.8, guest kernel 5.10.223
([benchmarks](../benchmarks.md)). External claims carry a URL; anything read
only from a search snippet, or not confirmed at all, is marked **unverified**.
Items marked **derived** are arithmetic on our own numbers, not measurements.
Nothing here was built or run; every gain below is a hypothesis with a stated
way to test it.

## Headline

1. The warm path is not at its floor. The VMM resumes in about 4 ms and an
   SDK `resume()` is 13 ms, but a pool-claimed `create()` is 23.6 ms (same as
   a cold one) and 26 ms to first command. Roughly 10 to 20 ms (derived) is
   daemon work after the resume, in series: health-check exec, MMDS write,
   optional egress, bookkeeping. No phase metric exists for it. This is the
   cheapest real latency win and it is unmeasured.
2. Density is bounded by things other than RAM. A guest declares 512 MiB but a
   cold-booted idle one dirtied only about 92 MiB (measured: allocated size of
   a diff `mem.bin`, [snapshot cost](snapshot-take-cost.md)). The daemon hands
   out guest addresses from one /24 (`free_hosts` is `2..=254` in
   `core/crates/vmm/src/network.rs`), and the tap pool defaults to 32. So a
   daemon tops out at 253 sandboxes plus held snapshots (each holds an address)
   however much RAM the box has, and 32 networked live sandboxes by default.
3. Firecracker's file-backed restore already shares clean pages between clones
   by construction: the memory file is a `MAP_PRIVATE` mapping, so unmodified
   pages are served from one host page-cache copy and only written pages are
   private ([snapshot docs](https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/snapshot-support.md),
   read 2026-11-25). Resumed sandboxes should therefore cost less RAM than
   cold-booted ones. That is a hypothesis we can measure in an afternoon and it
   decides most density work.
4. Techniques that look standard (KSM, huge pages, userfaultfd, io_uring block
   engine) are mostly wrong for this architecture, for reasons in
   [Rejected](#rejected). The two that fit are free page reporting and
   pressure-driven suspend.

## Where each number comes from

| Our figure | Reading | Source |
| --- | --- | --- |
| Warm create to first command 26.4 ms | `create_call` 23.6 + first command 2.4 | [benchmarks](../benchmarks.md#warm-pool-by-default-2026-11-24) |
| Resume 3.7-4.0 ms VMM, 13.1 ms SDK | the gap is HTTP plus daemon resume work | benchmarks |
| Cold 234 ms, init-mode image 147 ms | median to first command | benchmarks |
| Exec 1.76 ms per HTTP call, 245 us on an open connection | `vsock_client::call` opens a fresh UDS connection and `CONNECT` handshake per call | benchmarks, `core/crates/vmm/src/vsock_client.rs` |
| Parallel c8 20.1 creates/s, 357 ms median | 8 / 0.357 s is 22.4/s: throughput is concurrency over latency | suite result |
| Pool replenish: 2 parallel boots, guest must be 500 ms old before pause | each refill is boot + wait + snapshot, about 0.9 s (derived) | `pool_replenisher.rs`, `MIN_GUEST_AGE_BEFORE_PAUSE` |
| Resume cold cache vs warm | 138 vs 17 ms (full file), 85 vs 22 ms (diff file) | experiment E7, [snapshot cost](snapshot-take-cost.md) |

Two of those deserve a sentence. Throughput at c8 equals concurrency divided
by per-create latency, so the 20/s is a latency fact: either each create got
slower under contention (234 ms alone, 357 ms at c8, a 1.5x stretch) or the
box is simply CPU-bound by guest boots. A concurrency sweep tells which. And a
pool entry is consumed on claim (`claim_from_pool` resumes it and deletes the
snapshot), so sustained claims above about 2 per second (derived from the
replenish time) fall back to cold creates, whatever `default_warm_count` is.

## What the sources say

### Memory: balloon, reporting, hinting

| Fact | Source |
| --- | --- |
| Firecracker has a traditional balloon, plus free page reporting (guest reports unused ranges, VMM discards them, must be enabled before boot, runs continuously) and free page hinting (host-driven runs, developer preview). Both added in 1.14.0 as developer preview. | [ballooning.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/ballooning.md), [CHANGELOG](https://github.com/firecracker-microvm/firecracker/blob/main/CHANGELOG.md) |
| Reporting starts after "~2 seconds"; a hinting run took "~200 milliseconds for a 1GB VM" on average; the traditional balloon inflates at most 1 MiB at a time and can burn CPU chasing its target. No CPU-cost figure is given for reporting. | ballooning.md |
| Guest needs `CONFIG_VIRTIO_BALLOON`; reporting also needs `PAGE_REPORTING`, in ranges of at least 2 MiB (order 9 with 4 KiB pages). | ballooning.md |
| With the `File` restore backend each discarded range can add up to two mappings; too many can make Firecracker panic against the default `vm.max_map_count` of 65530 (about 128 MiB of scattered 4 KiB holes). Remedy: raise the limit or use the Uffd backend. | ballooning.md |
| Discarded ranges read as zero until rewritten; page zeroing after reclaim is where the kernel patch series attribute reporting's overhead. | ballooning.md; [LWN 805205](https://lwn.net/Articles/805205/) (search snippet, **unverified** detail) |

Reading: a Linux guest never gives freed memory back on its own, so a
long-lived sandbox's host footprint ratchets up to its high-water mark. A
fresh idle guest has little to return (its 92 MiB is in use). The gain is for
sandboxes that ran a build or a test suite and went quiet. No published
percentage of memory returned exists that I could find; the number has to
come from our workloads ([plan below](#density-measurement-plan)).

### Memory: sharing between clones

| Fact | Source |
| --- | --- |
| File backend is a `MAP_PRIVATE` mapping, loaded on demand, writes are copy-on-write; the file must outlive the VM and stay unmodified. | snapshot-support.md |
| KSM merges only anonymous pages, "never pagecache (file) pages", needs `madvise(MADV_MERGEABLE)`, defaults scan 100 pages per 20 ms, and "may use a lot of processing power". | [kernel KSM doc](https://www.kernel.org/doc/html/latest/admin-guide/mm/ksm.html) |
| Firecracker's host guidance: "Users should disable Kernel Samepage Merging" (page-deduplication side channels), disable swap, disable SMT where tenants must be separated. | [prod-host-setup.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/prod-host-setup.md) |
| Sandboxes of one function can share up to 85% of memory contents (80-90% across functions); content-based merging costs CPU. A search snippet reports CPU KSM raising co-runner tail latency 5.0-10.9x, against 1.6x with an accelerator. | [Medes, EuroSys 2022](https://jitaogithub.github.io/publication/medes/medes_paper.pdf); numbers from snippets, **unverified** |
| Reflinked files do not share page cache: kernel developers discussed it in 2016, 2019 and 2021 as unimplemented. Whether a current kernel does is **unverified**. | [lkml 2021 thread](https://lkml.iu.edu/hypermail/linux/kernel/2108.2/01971.html), snippet only |

Consequences. Clean pages of a resumed clone are file-backed, so KSM cannot
touch them; KSM would only act on a cold-booted guest's anonymous pages or on
dirtied pages, at a CPU cost and against Firecracker's own advice. Separately,
our per-sandbox rootfs is a reflink copy: cheap on disk, but if page cache is
per inode, N clones reading the same base file hold N host copies of it (and
each guest caches it again). A shared read-only base plus a small writable
layer would avoid that, at the price of an image-format change. Whether the
duplication is large is a measurement, not an assumption.

### Restore: prefetch, cache warmth, userfaultfd, huge pages

| Fact | Source |
| --- | --- |
| Snapshot-started functions ran 95% longer than memory-resident ones from page faults; recording the working set and prefetching it cut cold start 3.7x on average. | [REAP, arXiv 2101.09355](https://arxiv.org/abs/2101.09355) (abstract read) |
| Working set 3-39% of guest memory, 9% average; disk-resident snapshots up to 3.5x slower than page-cache-resident; minor faults average 3.7 us. | [FaaSnap, EuroSys 2022](https://www.sysnet.ucsd.edu/~voelker/pubs/faasnap-eurosys22.pdf), via [latency floor](latency-floor.md) (PDF not machine-readable here) |
| Huge pages: `2M` hugetlbfs "can improve boot times by up to 50%" in Firecracker's own tests, but restore with `2M` requires the Uffd backend and "dirty page tracking negates huge page benefits". `Transparent` was added in 1.17.0. | [hugepages.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/hugepages.md), CHANGELOG |
| Snapshot compression: up to 4.5x smaller and up to 55% faster restore with a hardware (de)compressor; a later paper reports software zstd with parallel decoders matched it for most benchmarks. | [Sabre, OSDI 2024](https://www.usenix.org/conference/osdi24/presentation/lazarev); second claim **unverified** |
| Firecracker changelog items that touch us: 1.17.0 adds `sync_snapshot_files`, `huge_pages` on load, faster vsock host-to-guest draining; unreleased work removes a 20-50 ms slowdown of restore and VM creation on Linux 6.18+ hosts; pre-allocating the fd table cut restores 30-70 ms in 1.5.0. | CHANGELOG |

Our own data already contains the page-cache result: a first resume costs 17
ms warm and 138 ms after the cache is dropped (full file), 22 and 85 ms for the
diff file. Our resumed first command is 6.3 ms, the load call 4 ms, so
working-set prefetch has at most a few milliseconds to win when the file is
warm. It matters only when the file is cold, and that is solved by reading the
file, not by a fault handler.

### Boot, CPU, storage, network, control plane

| Fact | Source |
| --- | --- |
| Serial console off saves "up to 70 ms"; a static NIC adds about 20 ms; a stock distro kernel adds about 900 ms; 150 microVMs/s per host, p99 146 ms at 50 concurrent. | [NSDI 2020](https://www.usenix.org/system/files/nsdi20-paper-agache.pdf), via latency floor |
| Skipping PCI enumeration saved 4-5 ms in a unikernel port; Firecracker's kernel policy says ACPI boot needs `CONFIG_PCI`. No controlled Linux comparison of trimmed against stock config was found. | [OSv blog 2019](http://blog.osv.io/blog/2019/04/19/making-OSv-run-on-firecraker/), [kernel-policy.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/kernel-policy.md), snippets |
| `io_engine: Async` (io_uring): uncached reads 1.5-3x more efficient and up to 30x IOPS, writes 20-45% more IOPS but less efficient, but device creation adds "up to ~110 ms", which "affects boot time and snapshot restore". Developer preview; open risks are worker PID exhaustion and workers not charged to the VM's cgroup. | [block-io-engine.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/api_requests/block-io-engine.md) |
| Network: about 25 Gbps ceiling at one emulation thread, about 0.06 ms virtualization latency; no queue-count or vhost option documented. vhost-net was declined upstream on attack-surface grounds. | [network-performance.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/network-performance.md), [issue 3707](https://github.com/firecracker-microvm/firecracker/issues/3707) (snippet) |
| vsock guest-to-host about 10 Gbps in a 2019 talk. | KVM Forum 2019 via a mirror, **unverified**, predates later fixes |
| Tokio: blocking pool defaults to 512 threads, 10 s keep-alive; workers default to core count. The daemon builds a default multi-thread runtime. | [tokio Builder docs](https://docs.rs/tokio/latest/tokio/runtime/struct.Builder.html) |
| cgroup CPU controls: `cpu.cfs_period_us` default 100 ms, quota and shares as the knobs; no pinning recommendation. | prod-host-setup.md |

## Ranked by expected gain per effort

Effort: S under 2 days, M under 2 weeks, L longer. Gain is stated against our
own numbers. "Measure" names a `examples/benchmark-suite` scenario; scenarios
marked **new** do not exist yet (today: `create`, `exec`, `snapshot`,
`parallel`).

| # | Technique | Expected gain | Effort | Risk | Measure |
| --- | --- | --- | --- | --- | --- |
| 1 | Phase-time and trim the pool-claim path (parallelize health check, MMDS write and egress; skip the check when resume and fixup succeeded) | warm create 26 ms toward 12-15 ms (**derived**: 4 ms resume + 2.4 ms first command + HTTP) | S | Low; keeps the health check as defence in depth for a bad snapshot, finding 20 | `create` paced; add `claim_phase_duration_ms{phase}` to `/metrics`; compare `create_call` median |
| 2 | Density harness (idle memory per sandbox, pool entry cost) | none directly; decides items 4 to 7 | S | None | **new** `density`; [plan](#density-measurement-plan) |
| 3 | Read memory files into page cache on claim and after daemon start (`posix_fadvise(WILLNEED)` or a read), and before an auto-resume | 60-120 ms off a cold-cache resume (measured 138 vs 17 ms full, 85 vs 22 ms diff); zero when warm | S | Low; evictable cache, bounded by pool size | **new** `resume` variant with the file evicted first (`resume_call_cold_cache`, `resume_first_command_cold_cache`) |
| 4 | Pressure-driven suspend plus admission by measured memory, not declared: when `MemAvailable` falls under a floor, snapshot the least recently active sandboxes (reuses auto-suspend); refuse creates when headroom is gone | idle sandboxes bounded by disk and addresses, not RAM; each suspended one frees its whole RSS; wake is 13 ms | M | Medium; a resumed sandbox is snapshotted in full (512 MiB file, 0.5-1.2 s), so disk and take time bite | `density` with `--mem-floor-mib`; metrics `suspended_to_free_mib`, `wake_call` |
| 5 | Free page reporting (balloon device, `PAGE_REPORTING` in the guest kernel) | returns what a finished build freed, up to hundreds of MiB per sandbox; about 0 for a fresh idle guest | M | Medium; developer preview, guest kernel config change, map-count limit on file-backed restores | `density` scenario `churn`: allocate 256 MiB, free, wait 5 s; metric `reclaimed_mib`; check `exec_roundtrip` and a memory-heavy command for regressions |
| 6 | Fan-out from one golden snapshot per shape instead of consuming one pool entry per claim (reuse the fork mechanism and `ResumeFixup`) | removes the refill (about 0.9 s CPU per claim) and the 2/s sustained-claim ceiling; burst creates cost about one resume (4-13 ms) each; N clones share clean pages; one memory file per shape instead of N | M-L | High; Firecracker calls resuming the same state more than once insecure unless uniqueness is restored, and a 5.10 guest has no VMGenID reseed. Needs a security review of per-clone fixups first | `parallel` with `--concurrency` 8, 16, 32 against a pool of 1 golden; `parallel_throughput_cN` |
| 7 | Burst-aware pool sizing (target from recent claim rate, cap by free addresses and disk) | a burst up to the pool size resumes at about 26 ms instead of cold 253 ms median (measured back to back) | M | Low-medium; costs disk and addresses held | `create` back to back and `parallel`; add a `pool_hit_ratio` metric |
| 8 | Finish the init-mode image as default | cold 232 to 147 ms; also less guest CPU per cold create, so more creates/s | S-M (blockers listed in the latency floor note) | Medium: drops systemd units | `create --image`, `parallel_throughput_c8` on both images (not yet measured on init) |
| 9 | Concurrency sweep to find the create ceiling | none; says whether 20/s is CPU, lock or lease bound | S | None | `parallel` at `--concurrency` 1, 2, 4, 8, 16, 32; plot throughput; `perf`/`pidstat` alongside |
| 10 | `vcpu_count=1` experiment (config only) | unknown; Firecracker says create and restore time depend on vCPU count and devices; fewer vCPU threads per VM | S | Low; guest parallelism | `create`, `resume` with `SANDKILN_VCPU_COUNT=1` alternating with 2 |
| 11 | Persistent per-sandbox vsock connection (or a small pool) for exec | up to about 1.3 ms of 1.76 ms per call (**derived**: 1.76 minus 0.245 minus HTTP share); an agent loop of 200 calls saves about 0.25 s | M | Medium; connections do not survive snapshot and the changelog shows repeated vsock-after-restore bugs | `exec_roundtrip` p50; **new** per-phase `exec_daemon_phase_duration_ms{phase}` first, since the split is unmeasured |
| 12 | Per-sandbox disk quotas (XFS project quota on the reflink volume) | protects density; no latency change | M | Low-medium | `density` with a writer that fills disk; expect a clean `ENOSPC` in the guest, no neighbours affected |
| 13 | Shared read-only base rootfs plus writable overlay layer | removes N host page-cache copies of the base, if they exist | L | Medium: touches drives, persistence, snapshot paths | `density` read test: N sandboxes read one 100 MiB file; watch host `Cached` |
| 14 | Trimmed guest kernel (`initcall_debug` first) | 10-30 ms estimate, **unverified**; guest kernel stage is 80-100 ms | M | Medium | `create_to_first_command` cold, init-mode image |
| 15 | zstd-compress archived `mem.bin` | disk density for the archive tier only; Sabre reports up to 4.5x on guest memory (**unverified** for ours) | S | Low; slower wake from archive | **new** `archive_resume`; measure ratio and decompress time |
| 16 | Move to Firecracker 1.17 or later | `sync_snapshot_files=false` for pool entries (20-110 ms off a take), vsock throughput fixes, THP option | M | Medium; every install must upgrade; interacts with diff snapshots | `snapshot` alternating versions |
| 17 | `cpu.weight` for background boots (replenisher) below claims | protects claim p95 during refills; unknown size | M | Low | `create` paced, p95 and max with and without |
| 18 | Working-set record and prefetch | at most a few ms (first command 6.3 ms, load 4 ms); REAP's 3.7x is against disk-resident, unwarmed restores | L | High | `resume_first_command` plus minor-fault count (below) |

Items 1 to 3 and 9 to 10 are measurements or one-file changes; every row from
4 on needs the `density` scenario first or a design decision.

## Top 5, in order

1. **Phase-time and trim the claim path (row 1).** The strongest evidence we
   have that a headline number is above its floor: `create_call` is 23.6 ms
   for a claim, 13 ms for a plain resume and 4 ms at the VMM. Add
   `claim_phase_duration_ms{phase="resume"|"health_check"|"egress"|"metadata"|
   "bookkeeping"}` next to `create_phase_duration_ms`, run the `create` scenario
   paced, then run independent steps concurrently. One suspect to test, not
   assume: the replenisher wakes on claim and starts two boots that compete for
   CPU with the claim itself.
2. **The density harness (row 2).** AGENTS.md section 10 says to measure the
   whole path before optimizing; density has no metric at all. It is also the
   only way to learn whether resumed sandboxes already share enough that
   rows 5 to 7 are unnecessary.
3. **Cache warmth for memory files (row 3).** One measured effect in our own
   data (85-138 ms cold, 17-22 ms warm) with a tiny change, and it protects
   every other claim: pressure-driven suspend (row 4) makes cold-cache wakes
   the common case, so this must land before it.
4. **Pressure-driven suspend and measured admission (row 4).** The only lever
   that turns "idle sandboxes per GiB" into "idle sandboxes per TiB of disk".
   It reuses auto-suspend and the 13 ms wake. Two prerequisites to design
   first: full snapshots of resumed sandboxes cost disk and 0.5-1.2 s each
   (the diff chain is the fix, listed as not built in
   [snapshot cost](snapshot-take-cost.md)), and the 253-address and 32-tap
   caps, which must become reported limits in `/status` and `kiln doctor`.
5. **Free page reporting (row 5), behind a flag, after the harness shows the
   ratchet.** If `churn` shows a guest holding most of its high-water mark
   after freeing it, reporting is the standard fix and Firecracker ships it. If
   the harness shows little retained, drop it.

Next after these: row 9 (the sweep, cheap and may redirect 6 and 7), then 7
and 6. Row 6 is the largest burst-throughput change but carries the only
security question in this note, so it waits for the sweep and a review.

## Rejected

| Idea | Reason |
| --- | --- |
| KSM | Merges only anonymous pages, so it cannot help clean file-backed clone pages; costs CPU with a measured tail-latency penalty in the literature; Firecracker's own host guidance says disable it (side channel). Revisit only if the harness shows a large duplicate share among cold-booted guests and a cheaper fix (rows 6 and 7) is ruled out. |
| Huge pages (`2M`, and `Transparent` for now) | Restore with `2M` needs userfaultfd; dirty tracking, which our diff snapshots rely on, "negates huge page benefits"; balloon frees no hugetlbfs memory. The up-to-50% boot gain is real in their tests but our default path restores or runs with tracking on. |
| userfaultfd restore backend | One host, warm cache, 4 ms load call: nothing to win; a crashed handler hangs the VM; conflicts with row 5's reasoning. Reconsider for remote or deduplicated memory (multi-node). |
| `io_engine: Async` by default | Adds up to about 110 ms to device creation, hitting boot and restore (our whole budget is 4-26 ms); developer preview; worker threads not charged to the VM cgroup, which undermines the jailer limits. Opt-in per sandbox for IO-heavy work could be revisited after GA. |
| Switching virtio-mmio to PCI | 1.13.0 made PCI opt-in; the benefit is hotplug and interrupt scaling, with no published boot or restore gain. Our guests run `pci=off`. |
| CPU templates | Exist to make hosts look alike for snapshot portability. Single-host, no measured latency gain. Needed only for multi-node. |
| vCPU pinning, vCPU hotplug | Pinning removes the scheduler's ability to overcommit idle vCPUs, the thing that makes density work; Firecracker does not recommend it. Hotplug: no Firecracker support found (**unverified**). |
| tap multiqueue, vhost-net | Not offered by Firecracker's network API as far as I found; vhost-net declined upstream. The data path is not our bottleneck (25 Gbps vs a 245 us exec). |
| Netlink crate for the tap lease, DNS cache | The lease is about 4 ms and concurrent; settled in [latency investigation](latency-investigation.md). |
| Raising `spawn_blocking` or worker counts | Defaults are 512 and core count; c8 uses 8 threads. Revisit only if row 9 shows a knee at a thread count. |
| Memory-compressing live guests | Needs a guest-side agent and CPU; Sabre-style compression is for snapshots at rest (row 15). |

## Density measurement plan

Goal: three numbers per host class: private memory per idle sandbox, sandboxes
that fit in N GiB, and the cost of one warm pool entry. No tool measures any
of them today. Run on an otherwise idle daemon, pool off except where stated,
after the box has a bigger tap pool and address space than the sweep needs
(`create-tap-pool.sh` and `SANDKILN_TAP_POOL_SIZE`; the /24 caps at 253).

**Definitions.** *Idle*: booted, one `true` command run, then 30 s with no
activity (Firecracker's own reporting delay is about 2 s, so 30 s lets it
settle). *Private memory*: `Pss_Anon + Private_Dirty` of the VMM process;
*shared*: `Pss_File` (the page-cache pages a resumed clone maps). Host cost:
`MemAvailable` drop plus the growth of `Cached` caused by the sandboxes.

**States, each measured with N = 1, 4, 16, 32, 64:**

| State | How reached | What is expected (hypothesis) |
| --- | --- | --- |
| A cold-booted idle | `create` with the pool off | about 92 MiB private plus VMM overhead (about 5 MiB is the NSDI figure, **unverified**) |
| B after churn | A, then allocate 256 MiB in the guest, free it, wait 30 s | RSS stays near high-water; falls when reporting is on |
| C resumed idle | claim from a pool, or `resume` a snapshot of A | much lower private memory; clean pages shared (`Pss_File`) |
| D suspended | `snapshot()` then stop | zero RAM; disk only |
| E pool entry | one replenished entry waiting | zero RAM except evictable cache; disk about 92 MiB sparse plus rootfs delta; one address |

**Procedure.** For each state and N: create N tagged sandboxes (the suite
already tags and cleans up by `benchmark_run`); sleep 30 s; sample
`/proc/meminfo`, `/proc/<pid>/smaps_rollup` for every Firecracker child of the
daemon, and `du -sk` of the state directory; in one guest per round also run
`free -m`. Fit a line through (N, host memory used); the slope is
*memory per idle sandbox*, the intercept is the daemon's own cost. Repeat A
and C three times, alternating, and keep every sample.

**Metrics to add to the suite result and report:**

| Metric | Meaning |
| --- | --- |
| `idle_private_mib_per_sandbox_{cold,resumed}` | slope of host memory against N, per state |
| `idle_shared_mib_per_sandbox_resumed` | `Pss_File` per resumed clone |
| `sandboxes_per_gib` | 1024 / private MiB, cold and resumed |
| `fit_in_gib(G)` | floor of (G x 1024 - intercept) / slope; also report the binding limit: RAM, disk, 253 addresses or tap pool |
| `retained_after_churn_mib` | state B RSS minus state A RSS, with and without reporting |
| `pool_entry_disk_mib`, `pool_entry_ram_mib` | `du` and PSS of one entry; compare with the 512 MiB the docs still quote |
| `pool_entry_refill_cpu_ms` | CPU consumed by one replenish (boot, settle, snapshot), from `/proc/<pid>` or the cgroup `cpu.stat`; multiplies by claim rate for the standing cost |
| `wake_call`, `wake_first_command` | resume of a state D sandbox, warm and cold cache |

**Cost of a warm entry, as a formula to fill in:** disk = allocated `mem.bin`
(diff of a cold-booted guest, 92 MiB measured with 0 MiB requested) + rootfs
divergence; RAM = evictable page cache of those files; one guest address;
CPU = `pool_entry_refill_cpu_ms` per claim. The warm-pool section of the
[benchmarks](../benchmarks.md#warm-pool-by-default-2026-11-24) and the
configuration page still describe an entry as "full-RAM, 512 MiB"; if the
harness confirms the sparse size, correct them.

**Where to implement.** The suite is client-side, so the host-side sampling
needs either a small script run beside it on the daemon's host
(`scripts/dev-tools/`, driven by a `density` benchmark when the daemon is
local, as the suite already detects) or a new `vmm_pid` on the sandbox record
so the suite can find processes without guessing. Add it to the suite and its
`compare` mode so a density regression fails like a latency one.

**Minor-fault count, one more cheap metric.** For row 18: after a resume and
the first command, read `minflt` and `majflt` from `/proc/<pid>/stat` of the
resumed Firecracker. If minor faults are in the hundreds, prefetch cannot pay.

## Not verified

- Any measured percentage of guest memory returned by free page reporting;
  only the mechanism and timings in the Firecracker docs.
- Whether current kernels share page cache between reflinked files (the
  discussions I found are 2016 to 2021).
- The REAP and FaaSnap quantitative sections, Medes' KSM tail-latency figures,
  and Sabre's relative result against software compression: PDFs would not
  extract, so those come from abstracts or search snippets.
- NSDI memory-overhead per microVM (I recall about 5 MB; not read).
- vsock throughput on a current Firecracker (the only figure is from 2019).
- Whether vCPU hotplug or tap multiqueue exist in Firecracker today.
- Every "derived" figure above; each is arithmetic on our measurements and
  none was run.
- Whether pool entries are always cold-booted with dirty tracking (so sparse);
  the code path suggests it, the harness will confirm.

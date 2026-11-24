# Benchmarks

All numbers are directional: one shared, actively used dev box (Intel
i9-13900K, kernel 6.8, ext4 unless noted, Firecracker v1.16.1), small
samples, not isolated hardware. Phase shares are consistent enough to act
on; absolute totals move a few percent run to run, and snapshot-take is the
most load-sensitive figure of all.

## Method

| Tool | What it measures |
| --- | --- |
| `cargo bench -p sandkiln-vmm --bench vm_lifecycle` | Cold boot, exec round trip, snapshot take, resume, against real Firecracker. Needs `SANDKILN_BENCH_FIRECRACKER_BIN`, `SANDKILN_BENCH_KERNEL_PATH`, `SANDKILN_BENCH_ROOTFS_PATH`. |
| `scripts/bench-report.sh [iterations]` | Fast regression check against `/metrics`, plus client-side `first_exec_client` (time to first real command after `create()` returns). Diffed against the last run. |
| `scripts/load-test.sh [concurrency] [iterations] [base-url]` | Concurrent create/exec/delete through the HTTP API (defaults: 10 workers, 20 iterations, port 7777). |
| `scripts/integration-test.sh [base-url]` | Full API behavior against a real daemon: sandbox lifecycle, tags, drives (persistence across sandboxes, conflict detection), snapshot/resume, auth, `/metrics`, error cases, in one repeatable run. Tracks and tears down everything it creates. See root `AGENTS.md`. |
| `scripts/dev-tools/profile-cold-create.sh` | 20 sequential creates with per-phase debug logging. |

Rules: measure the whole path before optimizing, and measure through to
"the guest answered a real request", not just to `create()` returning.
Log filter note: it is the binary name, `RUST_LOG=sandkilnd=debug`, not
`sandkiln_daemon=debug`, which silently matches nothing.

## Current numbers (2026-11-23/24)

| Metric | Value |
| --- | --- |
| Create to first command (bench-report) | 214.5ms (matrix: 237ms) |
| `POST /sandboxes` create, reflinked rootfs (XFS) | about 16-18ms |
| `POST /sandboxes` create, ext4 full copy | 187.5ms mean over 20 |
| Cold boot, spawn to `InstanceStart` | 12.6-14.0ms |
| Resume from snapshot | 3.7-4.0ms |
| Resumed first exec | 8.4ms |
| Exec round trip, open vsock connection | 242-248us (about 245us) |
| Snapshot take | 485-727ms (309.6-335.0 on a quieter day; up to 990ms under load) |
| Snapshot HTTP request | 0.56s (was 6.8s) |
| Graceful shutdown, 3 live sandboxes | 0.75s |
| Remaining boot floor | about 125ms systemd userspace (`dev-vda.device` 107ms, `fcnet.service` 45ms) |

## History: before and after

| Change | Metric | Before | After |
| --- | --- | --- | --- |
| API-socket wait: fixed 20ms sleep replaced by connect-with-backoff | socket wait | 20.11ms | 0.67-0.69ms |
| same | `boot_duration_ms` | 34.00ms | 11.22-11.44ms |
| same | cold create (ext4) | 167.65ms | 143.56-144.59ms (-14%) |
| same | criterion cold boot | 32.3-33.1ms | 10.5-10.9ms |
| Rootfs copy concurrent with network lease, `--reflink=auto` | load test create mean (4x5) | 369ms (p95 588) | 211ms (p95 577) |
| History store: WAL + `synchronous=NORMAL` | `record_created` | 1897.8us | 37.1us (-51x) |
| Native `FICLONE` clone on XFS (finding 4) | cold create | 187ms | 17.9ms |
| same | rootfs clone | 127.9ms | 0.08ms |
| Guest args `quiet loop.max_loop=1` (finding 22) | create to first exec | 413ms | 214.5ms (-48%) |
| same | resumed first exec | 18ms | 8.4ms |
| `force_stop` on paused VM (finding 13) | snapshot request | 6.8s | 0.56s |
| `iptables-restore` batching for egress | `egress_apply`, 6-CIDR policy | 8.3ms | 3.1ms (-63%) |
| Retry backoff in `Vm::call`, `open_pty`, `open_exec_stream` (flat 100ms sleep to 1-20ms backoff) | first exec | 420-460ms | 420-460ms (worst-case overshoot per retry 100ms to 20ms; invisible in noise) |

Earlier published numbers, kept for comparison: cold boot 10.5-10.9ms,
exec 220-250us, snapshot take 309.6-335.0ms, resume 6.5-7.7ms. The
2026-11-23 re-measure (this box, kernel 6.8) read cold boot 12.6-14.0ms,
exec 242-248us, snapshot take 485-727ms, resume 3.7-4.0ms; cold
`POST /sandboxes` mean over 20 was 187.5ms on ext4 with a full-copy clone and
18.4ms with a reflinked rootfs; the snapshot HTTP request was 6.8s before
finding 13 and 0.56s after.
Criterion on the older, pre-fix code: snapshot take about 322ms (309.6-335.0),
resume 25.8ms (25.5-26.2), cold boot 31.9ms (31.5-32.4).

### Load test, 4 workers x 5 cycles, 0 errors (8-tap pool, single node)

Create mean 369ms (p95 588ms, 5.59 cycles/sec) before the concurrent copy
plus `--reflink=auto`, 211ms (p95 577ms, 5.38 cycles/sec, exec mean 366ms,
delete mean 131ms) after. Throughput is flat because `create` is only one
phase of a cycle; samples are small and the box is shared. The `create`
improvement was later shown to be capped by the 4ms lease: see
[engineering post 20](../website/src/content/engineering/20-opt-concurrent-clone-and-lease.md).
Predates the socket-wait and retry fixes and has not been cleanly re-run.

## Per-phase profile of a cold create

20 sequential creates, ext4, 300MiB base rootfs, no jailer, 2 vCPU/512MiB,
empty body. Total 167.65ms (min 161.41, max 179.10), measured before the
socket-wait fix. The phases sum to 167.33ms, so the breakdown is complete.

| Phase | Mean | Share |
| --- | --- | --- |
| Rootfs clone (`cp --reflink=auto`) | 124.09ms | 74.0% |
| `Vm::boot` total | 34.00ms | 20.3% |
| of which process spawn | 0.34ms | 0.2% |
| of which wait for API socket | 20.11ms | 12.0% |
| of which 7 configuration PUTs | 1.61ms | 1.0% |
| of which `InstanceStart` | 11.87ms | 7.1% |
| History-store sqlite write | 9.24ms | 5.5% |
| Network lease (concurrent) | 4.36ms | about 0.1% of critical path |

Narrative and reasoning for each phase: the
[performance model post](../website/src/content/engineering/16-performance-model-and-measurement.md),
the [rootfs clone post](../website/src/content/engineering/17-opt-rootfs-reflink-clone.md),
the [socket wait post](../website/src/content/engineering/18-opt-boot-socket-wait.md)
and the [history store post](../website/src/content/engineering/19-opt-history-store-wal.md).
Kept here because they set decisions:
- The lease is about 4.4ms (three `ip`/`bridge` calls at about 1.4ms each) and
  runs concurrently with the clone, so it adds about 0.17ms to the critical
  path. A netlink crate would buy a couple of milliseconds at most and is not
  planned.
- Firecracker's seven config PUTs total 1.61ms; the slowest
  (`/network-interfaces/eth0`) is 0.51ms.

## A misread worth remembering

An early XFS experiment showed a real CoW clone (four 300MiB clones added
about 4MiB of disk; `cp --reflink=auto` about 110ms on ext4 to about 0ms on
XFS) but end-to-end create stayed at 160-170ms, and the first explanation
(the copy hidden behind the network lease, which is only about 4ms) was wrong.
The real cause was `clone_rootfs` always writing into `temp_dir()` on ext4.
Finding 4 fixed it (187ms to 17.9ms). The full story is
[engineering post 17](../website/src/content/engineering/17-opt-rootfs-reflink-clone.md)
and [post 20](../website/src/content/engineering/20-opt-concurrent-clone-and-lease.md).
`scripts/preflight-check.sh` warns when the base image and `rootfs_dir` do
not share a reflink filesystem. The XFS run moved only `SANDKILN_BASE_ROOTFS`
and the destination stayed on ext4, so a clone could never reflink; the
measurements were correct and only the causal story was guesswork. The fix
was scoped at the time as: make the clone destination configurable
(`SANDKILN_ROOTFS_CLONE_DIR` or similar, defaulting beside the base image),
surface whether each create reflinked or fell back (a `rootfs_clone` outlier
was otherwise indistinguishable from a slow disk), and re-run the XFS
experiment with the destination fixed. It was left out of the profiling pass
because it touches snapshot/archive path assumptions and its payoff cannot be
measured on an ext4-only box, the situation that produced the wrong
conclusion. That is finding 4 (the setting shipped as `SANDKILN_ROOTFS_DIR`).

## "Create returned" is not "ready"

Covered in [startup latency](../docs-site/architecture/startup-latency.mdx)
(first exec 420-460ms cold vs 3-5ms later vs 4-18ms resumed, the open
question of blocking `create()` until the agent answers) and
[engineering post 28](../website/src/content/engineering/28-opt-guest-boot-time.md).
Kept here: every benchmark before this finding stopped at `InstanceStart` or
`create()` returning, and the earlier pool framing (70-200ms vs 160-200ms)
compared only `create()` returning. `first_exec_client` in
`scripts/bench-report.sh`, timed client-side right after `create()` returns,
tracks it permanently next to the `/metrics` breakdown. It is an API-semantics
question, not only performance: today's fast `create()` figure is honest about
what it measures but easy to misread as "time until usable".

## Retry loops: a sibling of the socket-wait bug

`Vm::call`, `open_pty` and `open_exec_stream` retried a failed vsock call on
a flat `sleep(100ms)`; they now share one `retry_with_backoff` helper (1ms to
20ms for vsock) with `connect_api_with_retry`. The A/B showed first exec at
420-460ms for both versions: a real fix, invisible next to guest startup.
That led to the finding below. See
[engineering post 18](../website/src/content/engineering/18-opt-boot-socket-wait.md).

## Verification notes from the socket-wait fix

After that fix the full integration suite passed (snapshot, resume,
time-travel restore). One run in four failed a pool-claim MMDS assertion and
the next three were 300/300; the covered path (`Vm::update_metadata`) was
untouched, so it reads as the known intermittent resume failure (finding 20
family), stated as a reading of the evidence, not proven. `InstanceStart` also
read lower afterwards (11.87 to 8.75/9.13ms), which was not the target and is
not confidently attributed. Measured after, 2x20 creates: socket wait 20.11 to
0.67/0.69ms, `boot_duration_ms` 34.00 to 11.22/11.44ms, create 167.65 to
144.59/143.56ms (about 23ms, 14%).

Isolated sqlite A/B (20 sequential `record_created`, same disk) is not
directly comparable to the live 9.24ms figure; the removed double fsync is the
reproducible finding. Test
`open_sets_wal_and_synchronous_normal_for_write_latency` asserts the pragmas
took effect; 300/300 workspace tests and clippy clean at the time.

## Instrumentation left behind

- `/metrics`: `create_phase_duration_ms{phase="rootfs_clone"|
  "network_lease"|"setup"|"total"|"egress_apply"}` (`egress_apply` only
  recorded when a create requests a policy), `boot_duration_ms`,
  `rootfs_clone_total{method}`.
- Debug `tracing`: `"cold create setup phases"`, `"cold create complete"`
  (daemon), `"vm boot phase breakdown"` with per-endpoint PUT timings
  (vmm boot), `"attached tap device"` with each subprocess timed (vmm
  network).

## Snapshot and resume

- Snapshot take (pause, write memory and state to disk) is dominated by
  synchronously writing the full guest memory; a real cost for auto-suspend
  and any tiered idle lifecycle, not free. This dev box is a real, actively
  used desktop (editor, browser, remote-desktop client), not a dedicated
  bench machine, and the synchronous disk write is far more exposed to that
  than the other benchmarks. Re-runs measured 552.7ms (a 20-iteration bench-report run) and 560-915ms
  in isolation via `cargo bench -- snapshot_take`, with criterion reporting
  no detected change from its prior run (p=0.14): wide variance under real
  background load, not a regression or data error. The 322ms figure stands
  as a quieter-moment measurement; treat both as load-dependent.
- Resume was only about 19% faster than cold boot at the vmm layer (25.8ms
  vs 31.9ms; after the socket fix 6.5-7.7ms vs 10.5-10.9ms) because both
  are tiny on this lightweight image (small kernel, minimal agent init), not
  the dramatic win an earlier framing assumed. The real resume advantage
  appears when counted through to first command (see above).
- The gap between a 32ms boot and a 211ms create was never in boot or
  resume; it was per-create setup, almost entirely the rootfs clone. A
  pre-warmed pool's value is pre-doing that setup. See
  [pools and lineage](design-notes/pools-and-lineage.md).

## Not yet done

- Automating re-runs so regressions are visible over time (partly addressed
  by `scripts/bench-report.sh`).
- Re-measure the pool win now that a pool exists, rather than assuming it.
- `Vm::call` retry A/B, load-test table and the pool numbers predate several
  fixes and were not cleanly re-run.

- Concurrency-ceiling load test: see [load testing](design-notes/load-testing.md).
- Snapshot-cost reduction: see
  [latency investigation](design-notes/latency-investigation.md).

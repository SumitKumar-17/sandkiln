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
| [`examples/benchmark-suite`](../examples/benchmark-suite/README.md) (`scripts/dev.sh bench-suite`) | Client-side suite on the JS SDK against any daemon: create, first command, exec, snapshot, fork, resume, parallel create. JSON result plus a `compare` mode. See "Reproduce these numbers". |
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
| Create to first command, warm pool (default), paced | 26.4ms median, p95 39.4ms (cold: 233.9ms), see [Warm pool by default](#warm-pool-by-default-2026-11-24) |
| Remaining boot floor | about 125ms of systemd userspace in our guest (not to be confused with Firecracker's 125ms init bound; see "What each number measures") (`dev-vda.device` 107ms, `fcnet.service` 45ms); an opt-in init-mode image removes it, see below (median 147ms to first command instead of 230ms) |

## What each number measures

Guest-side reference, one `dmesg` capture (2026-11-24, default image, 5.10.223 guest): kernel clocksource switched to `tsc` at 0.022s, `systemd[1]` started at 0.107s of guest uptime. Firecracker's 125ms bound is for `InstanceStart` to init starting, so this guest is inside it; our 214ms to first command is a different, longer span.

The same word ("boot", "create", "first command") names different intervals in
different tools. Read a figure against this table before comparing it with
another. The long version is
[engineering post 29](../docs-site/engineering/29-what-a-start-latency-number-means.md).

| Figure | Interval it covers | Excludes | Statistic and source |
| --- | --- | --- | --- |
| Cold boot, `InstanceStart` (12.6-14.0ms) | Firecracker process spawn to `InstanceStart` returning: the host creating the VM and starting vCPUs | Guest kernel, init, agent | criterion, VMM level |
| `POST /sandboxes` (16-18ms reflinked) | Daemon work: clone, lease, spawn, configure, start | Guest boot; the agent is not yet listening | Mean from `/metrics`; suite median 21.2ms through the SDK |
| Create to first command (214.5ms) | `first_exec_client`: first real `exec` timed from the moment `create()` returned until it answers | The create call itself (add about 21ms for the suite's 235.4ms) | **Mean** over the `bench-report.sh` iterations, one box |
| `first_command_after_create` (213.1ms) | The same interval, client side through the SDK | The create call | **Median** of n=20, `examples/benchmark-suite` |
| Resume (3.7-4.0ms) | `PUT /snapshot/load` and resume, VMM level | HTTP hop, daemon bookkeeping, the guest's first command | criterion |
| `resume_call` (13.1ms; 13.6 and 14.4ms median in two n=15 loops after the tap lease moved to resume time, a different harness, see finding 29) | `Sandbox.resume()` through HTTP, including the daemon's resume work and the entropy and clock fixup | The first command | Median of n=20, suite |
| Resumed first exec (8.4ms), `resume_first_command` (6.3ms) | First command after a resume returned | The resume itself | Mean (`bench-report.sh`) and median (suite) |
| Exec round trip (about 245us) | One request on an already open vsock connection | HTTP, connection setup | criterion; the suite's 1.76ms is one HTTP request per command |
| about 125ms systemd userspace | Time systemd spends after the kernel hands over to it, inside our guest | Kernel boot | `systemd-analyze` in the guest. **Not** Firecracker's 125ms init bound, which is the time until `/sbin/init` *starts* |

Statistics used by the suite: the **median** is the middle sample (half faster,
half slower), the **p95** is the value 19 of 20 samples are at or below, **min**
and **max** are the extremes. Medians are reported because a single slow outlier
(a disk flush, another process on the shared box) moves a mean but not a median;
with n=20 the p95 sits next to the max and says little, so the median and min
are the stable figures. Every number here comes from one shared box.

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
[engineering post 20](../docs-site/engineering/20-opt-concurrent-clone-and-lease.md).
Predates the socket-wait and retry fixes and has not been cleanly re-run.

## Reproduce these numbers

[`examples/benchmark-suite`](../examples/benchmark-suite/README.md) reruns the
client-side figures against any daemon (`scripts/dev.sh bench-suite --iterations 20`)
and `compare` diffs two result files. A committed sample is in
[`examples/benchmark-suite/results/`](../examples/benchmark-suite/results/)
(2026-11-24, i9-13900K, kernel 6.8, guest kernel 5.10.223, n=20 per metric, 200
for exec, daemon idle at start, a second identical run agreed to within a few
percent except snapshot take). The suite times through the SDK, so it includes
the HTTP hop and the SDK; the published table above times lower layers for
several rows. Side by side, the published figure (left) against this sample's
median (right):

| Metric | Published | Suite median | Reading |
| --- | --- | --- | --- |
| Create to first command | 214.5ms (matrix: 237ms) | 213.1ms after `create()` returned, 235.4ms including the create call | Agrees. |
| `POST /sandboxes` create, reflinked | about 16-18ms | 21.2ms (18.5-25.4) | Slightly higher: the suite times the whole SDK call. Same order. |
| Resumed first exec | 8.4ms | 6.3ms | Agrees within noise. |
| Resume from snapshot | 3.7-4.0ms | 13.1ms `Sandbox.resume()` | Different layer: published is vmm-only, the suite includes the HTTP hop and all the daemon's resume work. Not a regression. |
| Exec round trip | about 245us | 1.76ms (min 537us) | Different layer: published reuses an open vsock connection, the suite makes one HTTP request per command. Not comparable. |
| Snapshot take | 485-727ms | 1208ms (min 1001, max 2615) | Materially higher. Unexplained: the suite times the full `snapshot()` call, the published figure is vmm-level, and this figure is the most load-sensitive. Treat as open until a vmm-level rerun on a quiet box reads the same. |
| Fork | not published | 8.8ms call, 7.5ms first command | New. |
| Parallel create, 8 at once | not published | 20.1/s, 357ms median per create to first command | New. |

The published table is left as it was. Where the suite and the published figure
measure the same thing (create to first command, resumed first exec) they agree.
Where they differ, the suite is the better-quality figure for what an SDK
caller experiences (it has a stated method, a recorded environment and the
raw samples), while the published vmm-level rows remain the right figures for
the cost of the VMM itself. Only snapshot take has a gap that is not explained
by the layer difference.

## Warm pool by default (2026-11-24)

`default_warm_count` (default 2) keeps warm instances of the daemon's default shape ready, so a plain `POST /sandboxes` resumes one. Measured with `examples/benchmark-suite` (`--only create --iterations 20`), same daemon and box, pool on against `SANDKILN_DEFAULT_WARM_COUNT=0`. Results are committed under `examples/benchmark-suite/results/`.

| Metric (median, n=20) | Pool off | Pool on, paced (`--pause-ms 4000`) | Pool on, back to back |
| --- | --- | --- | --- |
| `create_call` | 23.6ms | 23.6ms | 22.8ms |
| `first_command_after_create` | 209.7ms | 2.4ms | 231.0ms |
| `create_to_first_command` | 233.9ms (p95 244.4) | **26.4ms** (p95 39.4, max 144.3) | 252.7ms (min 24.2) |

How to read it:
- The `create` call takes the same 23-24ms either way, because it returns before the guest answers. What changes is whether the guest is ready at that point: cold, the first command still waits about 210ms for boot; warm, it answers in about 2ms.
- **Paced** is the steady state: a pause lets the replenisher refill the pool between creates, so every create is a resume. 233.9ms to 26.4ms, 8.9x faster.
- **Back to back** exposes the limit: 20 creates in a row drain the 2 warm instances (the first two hit, with a 24ms minimum) and the rest are cold while the pool refills, so the median is cold. A bigger `default_warm_count` or more spacing moves that boundary; it is a capacity setting, not a cap on speed.
- One paced sample was a 140.6ms create (the maximum); it is left in, cause not established.
- Cost: each warm instance is a held full-RAM snapshot on disk (512 MiB by default) and one guest address, and every claim triggers a replacement boot and snapshot in the background (replenishment pauses when addresses or disk run low, see `default_warm_count`).

### Replenish on claim (2026-11-24)

The pool replenisher now wakes the moment a claim drains a pool (it used to wait for a fixed 2-second tick) and refills up to two instances in parallel. Recorded with `examples/benchmark-suite` (`--only create --iterations 20`, `2026-11-24T1950-*` files under `examples/benchmark-suite/results/`):

| Pause between creates | create to first command, median (p95) |
| --- | --- |
| 4000 ms | 25.9ms (38.8) |
| 1500 ms | 27.2ms (49.5) |
| none, 20 back to back | 227.5ms (343.0); mostly cold, a pool of 2 cannot cover a burst of 20 |

There is no measurement of the 1500 ms case before this change, so no speedup is claimed for it; the table only records where the pool now keeps up. The product page reads its figure from the newest `warm-pool-on-paced` result.

## Init-mode image: the agent as PID 1 (2026-11-24)

Lever 1 of the [latency floor](design-notes/latency-floor.md). Same daemon,
same kernel, same base rootfs; the only difference is that the second image's
`/sbin/init` is the guest agent (`images/inject-agent.sh --init`, registered
as `init-agent`). `examples/benchmark-suite --only create --iterations 30
[--image init-agent]`, alternating default and init runs back to back on an
idle daemon (the suite refuses a busy one), client-side wall time:

| Metric (median, p95 in brackets) | Default (systemd), 4 runs of 30 | Init mode, 2 valid runs of 30 |
| --- | --- | --- |
| `create_call` | 20.8-22.7ms | 19.6-21.9ms (no change, as expected) |
| `first_command_after_create` | 204.5-212.5ms (p95 228-235) | 122.6-127.5ms (p95 134-143) |
| **`create_to_first_command`** | **225.5-234.6ms** (p95 248-258, min 205-219) | **144.6-149.7ms** (p95 156-167, min 123-131) |

About 83ms (36%) off the median, 232ms to 147ms pooled. The goal in the
latency note was 110ms; **it was not met**. Guest uptime at the first command
is 0.10-0.15s on the init image against 0.18-0.22s on systemd, and the
agent process starts at 0.08-0.11s of guest uptime, so what remains is mostly
the kernel reaching userspace (80-100ms with this CI kernel, including the
`ip=` configuration), not init. The next lever is the kernel (lever 5) and
moving the interface setup out of `ip=`.

Two measurement traps hit on the way, both recorded because they are easy to
repeat: (1) a first pair of init runs showed `create_call` 98ms instead of
22ms and a total of 222ms, a full copy of the 300MB image on every create, because
registering an image copies it into `images_dir`, which was on the ext4 root
volume (no reflink); those two runs are discarded (`rootfs_clones.copy` rose
by 60). (2) The suite measures create plus first command, so a faster `create`
alone would not have shown up. Not measured: the parallel benchmark on the
init image (another job held a live sandbox, so the suite refused to run
without `--allow-busy`, which would have skewed it) and the post-init
`initcall_debug` profile.

Correctness on the init image: the whole integration suite
(`SANDKILN_INTEGRATION_TEST_IMAGE=init-agent scripts/integration-test.sh`,
29 topics) passed 353 of 353, including exec, files, PTY, exec streaming,
previews and tunnels, egress, snapshots, fork, resume with the entropy and
clock fixup, and pools (pool creates and claims go through the same image).
The same suite on the default image passed 352 of 353 on its one run; the
failure was `18-pool`'s "deleting the pool also cleaned up exactly its own
warm snapshot (expected 1, got 2)", which passed on rerun on both images (a
flake against another job's snapshots on the shared box). That run did not cover
remote storage mounts, auth or the jailer; the later run below does, except the
jailer.
By hand on the init image: PID 1 is the agent, mounts match the systemd list
minus the systemd-private ones, `lo` is up, a double-forked orphan leaves no
zombie, and killing the agent is survived (init respawns it and the next exec
succeeds).

### Init-mode image, re-measured with warm pools off (2026-11-24)

A second, independent measurement on a private daemon (port 7792, own bridge and
taps, `default_warm_count = 0` so every create is cold, default 5.10.223 kernel,
reflinked `images_dir`), the agent rebuilt from the same tree for both images.
`examples/benchmark-suite --only create --iterations 20 [--image init-bench]`,
alternated default, init, default, init, default, init; six result files under
`examples/benchmark-suite/results/` named `*-default-image-cold-r*` and
`*-init-image-cold-r*`.

| Run | Default (systemd) `create_to_first_command` median (min) | Init mode median (min) |
| --- | --- | --- |
| 1 | 233.7ms (210.5) | 150.4ms (127.5) |
| 2 | 228.8ms (217.6) | 149.2ms (132.9) |
| 3 | 237.0ms (222.8) | 144.0ms (133.4) |
| Pooled, n=60 each | **231.7ms** (p95 258.6) | **147.5ms** (p95 161.2) |

The pooled gap is 84ms (36%), the same as the first measurement (232 to 147).
`create_call` is 21.3ms against 22.1ms; the whole difference is
`first_command_after_create` (210.5ms against 124.6ms). The milestone goal of
110ms is still not met.

Correctness on the same setup, with `auth_token` set (so the auth topic runs),
on a FUSE kernel with `rclone` baked into the image and a local
`rclone serve s3` endpoint on the bridge gateway (so `22-mounts` runs): the full
integration suite, 32 topics, 457 passed and 1 failed. The failure was
`29-snapshots-hold-no-taps` returning `503 no free tap devices` because this
daemon had only 6 taps and the suite ran two topics at once; the topic alone
passed 17 of 17. The jailer topic was not runnable (see finding 32).
A second full run on a real universal image built with
`build-universal-image.sh --agent ... --init` (Ubuntu 22.04, Node 24, Python 3.10,
4 agent users) passed 443 of 444 with the same single tap-pool miss (17 of 17
alone); in that guest `LANG`, users, sudo, node, python, git, DNS, HTTPS, `npm view`
and a pip-style HTTPS fetch behaved as in the systemd variant of the same image. Orderly
daemon shutdown with three live init-mode sandboxes (`scripts/restart-test.sh`
with `SANDKILN_INTEGRATION_TEST_IMAGE`): all three snapshotted, every Firecracker
child exited, all three came back as snapshots and one resumed and ran a command.

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
[performance model post](../docs-site/engineering/16-performance-model-and-measurement.md),
the [rootfs clone post](../docs-site/engineering/17-opt-rootfs-reflink-clone.md),
the [socket wait post](../docs-site/engineering/18-opt-boot-socket-wait.md)
and the [history store post](../docs-site/engineering/19-opt-history-store-wal.md).
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
[engineering post 17](../docs-site/engineering/17-opt-rootfs-reflink-clone.md)
and [post 20](../docs-site/engineering/20-opt-concurrent-clone-and-lease.md).
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
[engineering post 28](../docs-site/engineering/28-opt-guest-boot-time.md).
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
[engineering post 18](../docs-site/engineering/18-opt-boot-socket-wait.md).

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

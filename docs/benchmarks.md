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
| `scripts/integration-test.sh [base-url]` | Full API behavior against a real daemon. |
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
exec 220-250us, snapshot take 309.6-335.0ms, resume 6.5-7.7ms.
Criterion on the older, pre-fix code: snapshot take about 322ms, resume
25.8ms (25.5-26.2), cold boot 31.9ms.

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

Conclusions:
- The lease is about 4.4ms (three `ip`/`bridge` calls at 1.4ms each) and
  runs concurrently with the clone, so it adds about 0.17ms to the critical
  path. Batching them or adding a netlink crate would buy a couple of
  milliseconds at most. A netlink crate is not planned.
- Firecracker's seven config PUTs total 1.61ms; the slowest
  (`/network-interfaces/eth0`) is 0.51ms.
- The 20.11ms socket wait was a single quantized `sleep(20ms)`. The
  replacement retries the connect, not a file-exists check, because the
  socket file appears at `bind()` slightly before `listen()` and a fast
  existence poll would get `ECONNREFUSED`. Resume uses the same helper.
- The sqlite write sat on the critical path for a best-effort record;
  default `journal_mode=DELETE` + `synchronous=FULL` fsyncs twice. A unit
  test asserts the pragmas took effect.

## A misread worth remembering

An early XFS experiment (base rootfs on an XFS loopback) showed the CoW
clone was real (four 300MiB clones added about 4MiB of disk) but
end-to-end create stayed at about 160-170ms. The first explanation, that
the copy was hidden behind the network lease, was wrong: the lease is 4ms.
The real cause was that `clone_rootfs` always wrote into `temp_dir()` on
ext4, so the daemon never reflinked even though a standalone `cp` inside
XFS did. Finding 4 fixed it and the XFS re-run confirmed it (187ms to
17.9ms). `scripts/preflight-check.sh` now warns when the base image and
`rootfs_dir` do not share a reflink filesystem.

## "Create returned" is not "ready"

`POST /sandboxes` returning 200 did not mean the guest agent was
listening. A cold sandbox's first real exec measured 420-460ms, and every
later exec 3-5ms: a one-time guest-boot tax paid silently by the first
caller and misread as "exec is slow". A resumed sandbox skips it (first
exec 4-18ms, a 25-100x difference). `first_exec_client` in
`scripts/bench-report.sh` tracks it permanently.

Open API question: should the daemon block `create()` until the agent has
answered once, so 200 genuinely means ready? Needs a decision, not just a
fix.

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

- Snapshot take is dominated by synchronously writing the full guest
  memory. Re-runs under a busy desktop measured 552.7ms and 560-915ms
  (criterion reported no significant change, p=0.14), so treat every figure
  as load-dependent.
- Resume was only about 19% faster than cold boot at the vmm layer because
  both are tiny on this lightweight image. The real resume advantage
  appears when counted through to first command (see above).
- The gap between a 32ms boot and a 211ms create was never in boot or
  resume; it was per-create setup, almost entirely the rootfs clone. A
  pre-warmed pool's value is pre-doing that setup. See
  [pools and lineage](design-notes/pools-and-lineage.md).

## Not yet done

- Concurrency-ceiling load test: see [load testing](design-notes/load-testing.md).
- Snapshot-cost reduction: see
  [latency investigation](design-notes/latency-investigation.md).

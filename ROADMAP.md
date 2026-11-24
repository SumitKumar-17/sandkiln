# Roadmap

Current reality and the reasoning behind it. Every milestone ends with
something proven on real hardware, not just code that compiles. Detail,
findings and numbers live in [`docs/`](docs/README.md).

Last verified 2026-11-24 on the dev box (Intel i9-13900K, kernel 6.8,
Firecracker v1.16.1), on the final `main`: 450 Rust unit tests, clippy clean, and
the live suite (`scripts/integration-test.sh`, 442 checks over 34 topics) passed
442 of 442 on two full runs in a row, with the default warm pool on and the
current guest agent in the default images. The orderly-shutdown test
(`scripts/restart-test.sh`) and all 12 runnable examples
(`scripts/dev.sh examples-smoke`) pass.

Status key: **done** works end to end and is verified; **partial** works with
a stated gap; **planned** not built.

## Status by area

| Area | State | Notes |
| --- | --- | --- |
| Boot, exec, guest agent | done | Spawn to `InstanceStart` 12.6-14.0ms, exec round trip about 245us. |
| Daemon HTTP API (`sandkilnd`) | done | Create, exec, list, stop, file ops, env vars, graceful SIGTERM shutdown. |
| Networking | done | Tap pool, shared bridge, NAT, DNS proxy, bridge port isolation between sandboxes. |
| Snapshot, resume, fork | partial | Durable across restart, persistent-by-default stop, named sandboxes. Fork is single-live per snapshot. Take costs 310-990ms. |
| Time-travel restore, lineage | partial | Retired checkpoints (capped at 8) and parent pointers. Daemon API only, no SDK or CLI yet. |
| Idle lifecycle | partial | Auto-suspend, destroy and archive tier. Operator config only; no delete-after-archive. |
| Warm pools | done | `max_count` with queueing; a default pool (`default_warm_count`, 2) makes a plain create a resume, 26ms to first command. Explicit pool config is in memory only. |
| Drives and remote storage | done | Read-only sharing, S3-compatible mounts via rclone in the guest. |
| Egress policy | partial | IP/CIDR allow and deny per sandbox. No domain or port rules, IPv4 only. |
| Preview, tunnel, PTY, streamed exec | partial | HTTP preview only (no WebSocket proxy). PTY and exec-stream are in the JS/TS SDK, the Python SDK and the CLI; no live PTY resize, no exec kill. |
| Security | partial | Bridge isolation, ceilings, rate limits done. Jailer is opt-in and verified working on the dev box (finding 32 fixed). No seccomp. No seccomp. |
| Auth | partial | Single bearer token. Per-token scoping planned. |
| Images | partial | Register and boot prebuilt ext4 images. No OCI conversion, no managed catalog. |
| Observability | done | Structured logs, request ids, `/metrics`, JSON logs, console capture. `GET /status` shipped with `kiln status` and SDK `getStatus`/`get_status`. |
| History store | done | Sqlite record of every sandbox and how it ended. Live sandboxes do not survive a daemon crash. |
| JS/TS SDK, Python SDK (sync and async), CLI | done | Published to npm and PyPI (`sandkiln`, `sandkiln-cli`). Gaps are the ones named in the rows above. |
| MCP server (`sandkiln-mcp`) | done | Stdio server with 12 tools and 2 resources, published to npm (0.1.0) and verified from the registry against a live daemon. `sandbox_exec` uses the guest-enforced exec timeout. |
| Docs site, website, examples | done | Ten runnable examples. |
| GitHub Actions integration | partial | `sandkiln-run` action and example verified against a real daemon; the action also runs on GitHub-hosted runners against a stub daemon (2026-11-24: exit codes, cleanup, masking, cancellation), not yet against a real microVM there. Daemon inside a hosted runner is not supported ([note](docs/design-notes/ci-integration.md)). |
| Load testing | planned | Concurrency-ceiling ramp, see [load testing](docs/design-notes/load-testing.md). |
| Multi-node, regions | planned | Deliberately last. |
| GPU, desktop automation, git-native filesystem | planned / not planned | See [ideas not started](docs/design-notes/ideas-not-started.md). |

## Headline numbers

| Metric | Value |
| --- | --- |
| Create to first command | 214ms |
| `POST /sandboxes` create (reflinked rootfs) | about 16ms |
| Resume from snapshot | 3.7-4.0ms |
| Snapshot take | 485-727ms |

Directional, one shared box. Method, full table and history in
[benchmarks](docs/benchmarks.md).

## What's next

In order, most load-bearing first.

1. **Make the cold path the default-able one.** The init-mode image reaches
   the first command in about 147ms. Mounts, auth, PID 1 shutdown and a real
   universal image are verified on it; the jailer works (finding 32 fixed), and guest entropy, the cron/timesyncd loss and the parallel
   benchmark remain before it replaces systemd (go/no-go in the
   [latency floor](docs/design-notes/latency-floor.md)). Then per-image warm
   defaults and wake-on-claim replenishment. Plan: [feature plan](docs/design-notes/feature-plan.md).
2. **Admin tooling follow-up.** `GET /status`, `kiln status` and
   `getStatus`/`get_status` have shipped. `kiln doctor` has shipped as a
   client-side end-to-end probe (host prerequisite checks stay in
   `scripts/dev.sh preflight`). Remaining: a dashboard page on the same endpoint.
3. **Snapshot-take cost.** Measure sandkiln's own overhead, then evaluate
   Firecracker Diff snapshots. See
   [latency investigation](docs/design-notes/latency-investigation.md).
4. **Cold path.** The opt-in init-mode image reaches the first command in
   about 147ms against about 232ms with systemd (re-measured with the pool off);
   what is left is mostly kernel boot and `ip=` setup. Remaining steps before it
   can be the default, and the other ranked levers:
   [latency floor](docs/design-notes/latency-floor.md).
5. **Exec timeout for streamed exec.** `exec` takes a per-request
   `timeout_secs` that kills the guest process group (shipped; SDKs, CLI, MCP
   use it). `exec-stream` has no limit of its own: the GitHub Action runs the
   job under `timeout` in the guest instead.
6. **Reflink by default** for production setups (preflight already warns).
   `create-reflink-volume.sh` is a loop mount; add a systemd mount unit.
7. **Jailer follow-ups.** It works (finding 32 fixed: supervisor, shared jail
   group, group-owned taps). Left: snapshots of jailed sandboxes, per-uid taps
   to close the shared-group tap risk, a boot-time unit for the cgroup
   delegation. Also a VM-exit watcher so a guest that shuts itself down is not
   left listed (finding 33, fixed).
8. **Split oversized daemon files** (finding 6) per AGENTS.md section 2.
9. **Website:** build and screenshot-review every engineering post.
10. **Housekeeping:** the dev box XFS volume at `/mnt/sandkiln-xfs` needs
   `create-reflink-volume.sh` after a reboot; delete the stale `pass.txt` in
   the remote repo dir and rotate the box password; bump JS tooling
   (finding 7).

Smaller deferred items are listed in the design notes for each area
(client surface for time-travel and lineage, PTY resize, exec kill, domain
and port egress rules, per-token auth scoping, daemon-restart pool state).

## Engineering principles

- **Modular.** Each concern is its own crate or package with a narrow public
  API; no crate reaches into another's internals.
- **Benchmark the hot paths.** Boot, exec, snapshot and resume time, measured
  through to "the guest answered", never just to `create()` returning.
- **Prove it.** A milestone ends with something run on real hardware.
- **No half-finished surfaces.** A feature works end to end or is not
  claimed. Deferred work stays marked planned.

Development happens in this repo. Anything needing KVM, a Linux toolchain or
real hardware runs on the remote dev box via `scripts/remote.sh`. Full
standard: [AGENTS.md](AGENTS.md).

## Where the detail went

| Topic | File |
| --- | --- |
| Audit findings 1-24 | [docs/findings.md](docs/findings.md) |
| Benchmarking | [docs/benchmarks.md](docs/benchmarks.md) |
| Persistence and snapshotting | [docs/design-notes/snapshots.md](docs/design-notes/snapshots.md) |
| Warm pools, snapshot lineage | [docs/design-notes/pools-and-lineage.md](docs/design-notes/pools-and-lineage.md) |
| Drives and remote storage | [docs/design-notes/drives-and-storage.md](docs/design-notes/drives-and-storage.md) |
| Firewall and egress policy | [docs/design-notes/egress-firewall.md](docs/design-notes/egress-firewall.md) |
| Security hardening, multi-agent isolation, privileged workloads | [docs/design-notes/security-hardening.md](docs/design-notes/security-hardening.md) |
| Dev servers and live preview (preview, tunnel, PTY, exec-stream) | [docs/design-notes/preview.md](docs/design-notes/preview.md) |
| Tags and sandbox metadata | [docs/design-notes/tags-and-metadata.md](docs/design-notes/tags-and-metadata.md) |
| Observability | [docs/design-notes/observability.md](docs/design-notes/observability.md) |
| Client SDKs, CLI | [docs/design-notes/sdks-and-cli.md](docs/design-notes/sdks-and-cli.md) |
| Authentication, base and custom images, networking, documentation and examples | [docs/design-notes/images-and-auth.md](docs/design-notes/images-and-auth.md) |
| Load testing | [docs/design-notes/load-testing.md](docs/design-notes/load-testing.md) |
| Further latency investigation | [docs/design-notes/latency-investigation.md](docs/design-notes/latency-investigation.md) |
| Multi-node, ecosystem, ideas not started | [docs/design-notes/ideas-not-started.md](docs/design-notes/ideas-not-started.md) |

Other references: [SELF_HOSTING.md](SELF_HOSTING.md),
[CHANGELOG.md](CHANGELOG.md), `docs-site/` (user docs), `examples/AGENTS.md`.

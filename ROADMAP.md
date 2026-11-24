# Roadmap

Current reality and the reasoning behind it. Every milestone ends with
something proven on real hardware, not just code that compiles. Detail,
findings and numbers live in [`docs/`](docs/README.md).

Last verified 2026-11-24 on the dev box (Intel i9-13900K, kernel 6.8,
Firecracker v1.16.1): 345 unit tests, clippy clean, live suite
(`scripts/integration-test.sh`) 334 of 335 and 333 of 335 on the last two
full runs. The misses were the finding 20 pool flake (fixed since) and a timing bound
that has since been loosened.

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
| Warm pools | done | `max_count` with queueing. Config is in memory only. |
| Drives and remote storage | done | Read-only sharing, S3-compatible mounts via rclone in the guest. |
| Egress policy | partial | IP/CIDR allow and deny per sandbox. No domain or port rules, IPv4 only. |
| Preview, tunnel, PTY, streamed exec | partial | HTTP preview only (no WebSocket proxy). PTY and exec-stream are JS/TS and CLI only; no live PTY resize, no exec kill. |
| Security | partial | Bridge isolation, ceilings, rate limits done. Jailer is opt-in and unverified on hardware. No seccomp. |
| Auth | partial | Single bearer token. Per-token scoping planned. |
| Images | partial | Register and boot prebuilt ext4 images. No OCI conversion, no managed catalog. |
| Observability | done | Structured logs, request ids, `/metrics`, JSON logs, console capture. `GET /status` shipped with `kiln status` and SDK `getStatus`/`get_status`. |
| History store | done | Sqlite record of every sandbox and how it ended. Live sandboxes do not survive a daemon crash. |
| JS/TS SDK, Python SDK (sync and async), CLI | done | Published to npm and PyPI (`sandkiln`, `sandkiln-cli`). Gaps are the ones named in the rows above. |
| MCP server (`sandkiln-mcp`) | done | Stdio server with 12 tools and 2 resources, published to npm (0.1.0) and verified from the registry against a live daemon. Exec timeout is client-side only (see the exec-timeout item). |
| Docs site, website, examples | done | Ten runnable examples. |
| GitHub Actions integration | partial | `sandkiln-run` action and example verified against a real daemon; not yet run on GitHub. Daemon inside a hosted runner is not supported ([note](docs/design-notes/ci-integration.md)). |
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

1. **Re-run the full suite twice** to confirm finding 20's fix (25 of 55 early-snapshot resumes crashed before, 0 of 180 after) holds at suite scale.
2. **Admin tooling follow-up.** `GET /status`, `kiln status` and
   `getStatus`/`get_status` have shipped. `kiln doctor` has shipped as a
   client-side end-to-end probe (host prerequisite checks stay in
   `scripts/dev.sh preflight`). Remaining: a dashboard page on the same endpoint.
3. **Snapshot-take cost.** Measure sandkiln's own overhead, then evaluate
   Firecracker Diff snapshots. See
   [latency investigation](docs/design-notes/latency-investigation.md).
4. **Guest userspace boot.** About 125ms of systemd userspace, not to be confused with Firecracker's 125ms init bound (`dev-vda.device`
   107ms, `fcnet.service` 45ms) is the remaining floor to first command: a
   purpose-built init or trimmed image. Ranked levers and goals:
   [latency floor](docs/design-notes/latency-floor.md).
5. **Per-request exec timeout** that also kills the guest process (today the
   process keeps running after a 504 or an MCP/CI client timeout).
6. **Reflink by default** for production setups (preflight already warns).
   `create-reflink-volume.sh` is a loop mount; add a systemd mount unit.
7. **Split oversized daemon files** (finding 6) per AGENTS.md section 2.
8. **Website:** build and screenshot-review every engineering post.
9. **Housekeeping:** the dev box XFS volume at `/mnt/sandkiln-xfs` needs
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

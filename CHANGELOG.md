# Changelog

Tracks notable changes across the whole project (daemon, core crates,
clients). The JS/TS SDK is published as [`sandkiln` on
npm](https://www.npmjs.com/package/sandkiln) and the CLI as
[`sandkiln-cli`](https://www.npmjs.com/package/sandkiln-cli) (installs
the `kiln` command) — both versioned together below, since every CLI
release depends on the SDK release it was built against. Everything else
(daemon, core crates, Python SDK) doesn't have its own release yet; where
a change only affects one of those, it's called out explicitly instead
of implying it shipped to npm. Format loosely follows [Keep a
Changelog](https://keepachangelog.com/).

## Unreleased

### Added
- Tiered idle lifecycle, archive tier (daemon only — no SDK/CLI change,
  nothing to publish): `SANDKILN_ARCHIVE_TIMEOUT_SECS` moves a held
  snapshot's `state.snap`/`mem.bin` onto a separately configured
  `SANDKILN_ARCHIVE_DIR` once it's sat unresumed that long, applying to
  any held snapshot regardless of how it arose. `GET /snapshots` reports
  `archived_at_unix`. Live-verified end to end, including a real
  Firecracker constraint found mid-build (the rootfs backing file's path
  is baked into `state.snap` with no override at resume time — confirmed
  by an actual resume failure after moving it — so only `state.snap`/
  `mem.bin` move, `rootfs_path` never does) and a real latent bug it
  surfaced (two existing snapshot-cleanup code paths derived the
  directory to remove from a hardcoded hot-root path instead of the
  snapshot's own current location, which would have silently leaked an
  archived snapshot's files forever). See `ROADMAP.md`'s "Persistence and
  snapshotting" section for the full story. 1 new
  `scripts/integration-test.sh` check (the archive-after-timeout
  behavior itself needs a differently-configured daemon, verified
  manually instead — same tradeoff already made for the sqlite-history
  restart case), 245/245 passing overall.
- Pre-warmed pools: `max_count` ceiling with queueing (daemon only — no
  SDK/CLI change, nothing to publish). `PoolConfig.max_count` caps the
  total live instances (warm + claimed) a pool's profile may have at
  once; a `POST /sandboxes` claim arriving at the ceiling with nothing
  warm queues (a `tokio::sync::Notify` per pool) for up to 30s before a
  real `503`, instead of rejecting outright or silently exceeding the
  ceiling. `GET /pools` now also reports `max_count`/`claimed`.
  Live-verified: blocks a second claim at capacity, wakes it the instant
  a slot frees (not on a poll interval), and 503s cleanly after the full
  30s when nothing frees up. Also fixed a real bug found by live-testing
  this exact feature: a failed warm claim's cold-create fallback wasn't
  being attributed back to the pool, letting `max_count` be silently
  exceeded under this project's own already-documented high resume
  failure rate — fixed with a bounded 3-attempt retry so the fallback
  re-resolves pool capacity instead of falling through unattributed. See
  `ROADMAP.md`'s "Persistence and snapshotting" section for the full
  story. 8 new `scripts/integration-test.sh` checks, 244/244 passing
  overall.
- Pre-warmed pools (0.7.0, above) added to the Python SDK too
  (`Pool.create/list/delete`, `packages/python/src/sandkiln/pool.py`) —
  not published to PyPI yet, so nothing to version here, but live-verified
  end to end against a real daemon the same way the JS/TS SDK version was.
- Guest-accessible metadata service (daemon/`sandkiln-vmm` only — no
  SDK/CLI change, nothing to publish): every sandbox with a name and/or
  tags now automatically serves its own `{id, name, tags}` via
  Firecracker's native MMDS at `http://169.254.169.254/` inside the
  guest, V2 (token-gated). Live-verified by curling it from inside a
  real sandbox — 5 new `scripts/integration-test.sh` checks, 194/194
  passing. See `ROADMAP.md`'s "Tags and sandbox metadata" section for
  the `Accept: application/json` header requirement and other details.
- Durable sandbox history (daemon/new `sandkiln-store` crate only — no
  SDK/CLI change, nothing to publish): `GET /sandboxes/history`
  (`?live_only=`, `?limit=`) backed by a new sqlite database, surviving
  a daemon restart unlike `GET /sandboxes`. Explicitly does **not** mean
  live sandboxes survive a restart — nothing can make that true today —
  see `ROADMAP.md`'s "Tags and sandbox metadata" section for exactly
  what this does and doesn't solve, and the real restart test that
  verified it (a live record correctly flips to `orphaned_by_restart`
  with its tags intact; already-ended records are untouched). 15 new
  `scripts/integration-test.sh` checks, 209/209 passing overall.
- PTY WebSocket sessions (0.6.0, above) now covered by
  `scripts/integration-test.sh` itself (`17-pty.sh`) via a small Node
  helper (`scripts/lib/pty-check.mjs`) — a real command's output
  round-tripping through a real shell, plus a real check that the
  SIGHUP-on-hangup guest-agent fix actually prevents an orphaned shell
  after a client disconnects without exiting one. 6 new checks, 220/220
  passing overall (with `SANDKILN_AUTH_TOKEN` set — auth-gated cases
  bring the total above 209 too).

## [0.7.0] — 2026-09-08

### Added
- Pre-warmed snapshot pools: `POST/GET /pools`, `DELETE /pools/:id`
  (daemon), `Pool.create/list/delete` (JS/TS SDK), `kiln pool create|ls|rm`
  (CLI). A plain `POST /sandboxes`/`Sandbox.create()` (no `drives`, no
  `rateLimit`) matching a configured pool's image/resources
  transparently resumes a warm snapshot instead of cold-booting, once
  the background replenisher has one ready — no separate "create from
  pool" call. Live-verified with a real measured win on a clean claim
  (roughly 70–200ms vs. a cold create's ~160–200ms) — but that clean case
  is honestly **not** the reliable common case on this dev box today: see
  `ROADMAP.md`'s "Persistence and snapshotting" section for two real
  Firecracker/KVM findings this surfaced and exactly how each is
  handled — a guest-kernel-panic-on-resume failure mode measured at
  roughly 1-in-3 to 2-in-3 resumes (not rare), handled with a real
  post-resume health check that falls back to a normal cold create
  rather than ever handing back a broken sandbox, and MMDS not
  surviving snapshot/restore. 19 new `scripts/integration-test.sh`
  checks (`18-pool.sh`), 234/234 passing overall.

## [0.6.0] — 2026-09-08

### Added
- Interactive terminal access: `Sandbox.pty()` (JS/TS SDK, returns a
  native `WebSocket`) and `kiln sandbox pty <id>` (CLI, raw terminal
  mode) open a live, bidirectional shell session inside a sandbox over
  `GET /sandboxes/:id/pty`, distinct from batch `exec`. A new
  `examples/interactive-terminal` reference project replaces ad hoc
  testing for this one. Live-verified end to end on real hardware,
  including a real bug found and fixed during that verification: the
  guest agent originally left one side of a disconnected session blocked
  forever (or, in the other direction, an orphaned shell process running
  with nothing attached to it) — fixed with an explicit socket shutdown
  and a `SIGHUP` to the shell's process group, matching real terminal
  hangup semantics. See `ROADMAP.md`'s "Dev servers and live preview"
  section for the auth (`?token=`), the per-sandbox 64-session cap, and
  the no-live-resize caveat.

## [0.5.0] — 2026-09-08

### Added
- Persistent drives exposed in both SDKs and the CLI for the first time
  (`Drive.create/list/delete`, attach at create time via `drives`/
  `--drive <id[:ro]>`) — previously daemon-HTTP-API-only.
- Full filesystem operations: `chmod`/`chown`/`mkdir`/`rename`/`copy`/
  `symlink`/`readlink`/`truncate`/directory listing with metadata, as
  new vsock protocol commands plus daemon routes, both SDKs, and the
  CLI. Also exposes `list_dir`, which existed in the protocol/guest
  agent already but was never wired up above that layer. **Live-verified
  end to end** after rebuilding and re-injecting the guest agent: 21 new
  `scripts/integration-test.sh` checks, 193/193 passing — see
  `ROADMAP.md` for exactly what's covered.

### Changed
- Reorganized `scripts/`: one-time host-provisioning steps
  (`create-tap-pool.sh`, `grant-net-admin.sh`, `install-firecracker.sh`,
  `allow-passwordless-cap-grant.sh`, `start-dns-proxy.sh`) moved into
  `scripts/host-setup/`; narrow manual-debugging tools
  (`boot-test-vm.sh`, `setup-tap-network.sh`) moved into
  `scripts/dev-tools/`. Every internal call site and doc reference was
  updated. `preflight-check.sh`, `setup.sh`, `sandkilnd-ctl.sh`,
  `install-systemd-service.sh` (+ its template), `integration-test.sh`,
  `load-test.sh`, `remote.sh`, and the new `dev.sh` dispatcher all stay
  at the top level — no code deleted, purely a reorganization.
  **Action needed on any host that already ran
  `allow-passwordless-cap-grant.sh` under its old path**: that sudoers
  rule is keyed to the exact old path and won't match the new one —
  re-run `sudo scripts/host-setup/allow-passwordless-cap-grant.sh` once,
  interactively, to restore passwordless `CAP_NET_ADMIN` granting on
  daemon restart. **Verified live** on the dev box after applying that
  fix: `sandkilnd-ctl.sh restart` grants the capability cleanly again,
  and a full `scripts/integration-test.sh` run with `SANDKILN_AUTH_TOKEN`
  set passes 167/167 (154 base + 8 rate-limit + 5 auth checks).

## [0.4.0] — 2026-09-08

### Added
- Per-sandbox I/O rate limiting via Firecracker's own token-bucket rate
  limiter: `POST /sandboxes` (and `get-or-create`) accepts an optional
  `rate_limit: {bandwidth_bytes_per_sec?, ops_per_sec?}`, applied
  uniformly to the rootfs drive, every attached drive, and both
  directions of the network interface. At least one sub-field must be
  set and non-zero if `rate_limit` is present at all — `0` or an empty
  object is rejected with `400`, the same convention as `vcpu_count`/
  `mem_size_mib`. `None`/omitted means unlimited host I/O, unchanged
  from before this existed. Both SDKs (`rateLimit`/
  `rate_limit_bandwidth_bytes_per_sec`+`rate_limit_ops_per_sec`) and the
  CLI (`--rate-bandwidth`/`--rate-ops` on `kiln sandbox create` and
  `get-or-create`) are updated to match. Not yet exposed as a
  standalone daemon-level ceiling/floor — see `ROADMAP.md`'s Security
  hardening section.

## [0.3.0] — 2026-09-01

Custom/managed images, named sandboxes, persistent-by-default stop,
read-only shared drives, and auto-suspend on idle — all new,
backward-compatible additions, nothing removed or changed.

### Added
- Custom/managed base images: `POST /images` registers an already-built
  ext4 rootfs from a host path (`GET /images` to list, `DELETE /images/:id`
  refused while anything still references it); `POST /sandboxes` accepts
  an `image_id` to boot from one instead of the daemon's default rootfs.
  The daemon can't verify the guest agent is baked in (runs unprivileged,
  no loop-mount) — `scripts/preflight-check.sh --root-checks --rootfs-image
  <path>` does that check out of band. Both SDKs (`Image.register/list/
  delete`, `imageId`/`image_id` on `create`) and the CLI (`kiln image
  ls|create|rm`, `kiln sandbox create --image`) are updated to match.
- Named sandboxes and persistent-by-default stop: `DELETE /sandboxes/:id`
  now auto-snapshots on stop by default instead of destroying (`?keep=false`
  or `kiln sandbox rm --destroy` opts back into the old hard-destroy
  behavior); sandboxes can carry a caller-given `name` (unique among live
  sandboxes) via `POST /sandboxes`, resolved with `GET
  /sandboxes/by-name/:name` (live only) or resumed/created in one
  race-safe call with `POST /sandboxes/get-or-create`. Both SDKs and the
  CLI (`--name`, `sandbox get-or-create`, `sandbox get`, `rm --destroy`)
  are updated to match.
- Auto-suspend on idle (`SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`): an idle
  sandbox is paused and snapshotted instead of destroyed, freeing its
  VM/network while keeping it resumable. `SANDKILN_IDLE_TIMEOUT_SECS`
  (plain destroy) still works as before and now acts as a backstop when
  both are set (must be strictly longer). `GET /snapshots
  ?source_sandbox_id=<id>` and matching SDK/CLI methods find what a
  vanished sandbox turned into.
- Read-only shared drives: a drive attached read-only may now be attached
  to arbitrarily many sandboxes (and held snapshots) concurrently; any
  read-write attachment, existing or requested, still needs exclusive
  access. **API shape change**: `GET /drives`/`POST /drives`'s
  `attached_to` field changed from `Option<String>` to a list of holders
  (each with a `read_only` flag) — not yet published to any SDK, so no
  external break, but worth knowing if you're consuming the daemon's raw
  HTTP API directly.

### Fixed
- `scripts/integration-test.sh` grew 65 checks covering all of the above
  (154 total, up from 89) — running it for real against live hardware
  after this release caught two genuine bugs, both fixed: five of the
  script's own `DELETE /sandboxes/:id` calls predating persistent-by-
  default stop still expected the old full-destroy response and needed
  an explicit `?keep=false`; and `POST /sandboxes/get-or-create` with no
  `name` field returned an undocumented `422` instead of a clean `400`
  (missing `#[serde(default)]` let axum's own JSON rejection fire before
  the daemon's validation ever ran).

## [0.2.0] — 2026-08-30

`resume`/`fork`/`snapshot`/`previewUrl` and `vcpuCount`/`memSizeMib`
overrides — all new, backward-compatible additions to the SDK. First
publish of the CLI, as `sandkiln-cli` rather than the obvious `kiln`
(already an unrelated package on npm) — `npm install -g sandkiln-cli`
still gives you the `kiln` command.

### Added
- Persistent drives (`POST /drives`, attachable at sandbox creation),
  with cross-sandbox conflict detection and persistence across sandboxes
  verified live.
- Snapshot/resume/fork: save a running sandbox's full state to disk and
  boot a new sandbox from it, either consuming the snapshot (`resume`) or
  not (`fork`, so the same state can be resumed repeatedly). Durable
  across a daemon restart (metadata persisted atomically, reconciled at
  startup). Exposed through both SDKs and `kiln`.
- Host-side reverse proxy for dev-server preview
  (`/sandboxes/:id/preview/:port`), token-in-query-param auth for
  browser/iframe use, `Sandbox.previewUrl()` in both SDKs, `kiln sandbox
  preview`, and an `examples/dev-server-preview` reference.
- Per-sandbox resource overrides (`vcpu_count`/`mem_size_mib` on
  `POST /sandboxes`) with enforced, configurable ceilings.
- Automatic idle-timeout reaper (`SANDKILN_IDLE_TIMEOUT_SECS`).
- Opt-in Firecracker jailer support (chroot, cgroup v2 limits, a
  dedicated uid/gid per VM) via `SANDKILN_JAILER_ENABLED`.
- Request-id correlation (caller-supplied or generated `X-Request-Id`)
  threaded through every VM operation an HTTP request triggers; a
  `/metrics` endpoint (Prometheus text format); `SANDKILN_LOG_FORMAT=json`;
  guest serial console captured to a per-VM log file.
- Universal base rootfs image build (`images/build-universal-image.sh`):
  Ubuntu, current Node.js/Python, common tooling, multi-agent isolation
  users, built reproducibly rather than a fetched test image.
- `scripts/preflight-check.sh`, `scripts/sandkilnd-ctl.sh`, and
  `scripts/install-systemd-service.sh` + a real systemd unit template —
  a tested, reproducible self-hosting path, written up in full in
  `SELF_HOSTING.md`.
- `kiln` CLI first published to npm (as `sandkiln-cli`), and the CLI/SDK
  publish workflows gained signed provenance.

### Fixed
- Preview URLs 404ing on every SDK-generated URL: axum's `*path` wildcard
  doesn't match a bare trailing slash with nothing after it, which is
  exactly what `previewUrl()`'s default path produces — added the missing
  explicit route.
- Snapshots living only in memory, so a daemon restart made every
  existing snapshot's files permanently unreachable through the API even
  though the bytes were still on disk — fixed by persisting metadata
  atomically and reconciling it at startup.

## [0.1.0] — 2026-08-26

First publish. The core execution primitive plus a full first pass of
lifecycle, networking, auth, and tooling.

### Added
- Core execution primitive: Firecracker microVM boot, a guest agent
  (vsock exec / read_file / write_file / list_dir), and a host-side
  vsock client, all proven end to end on real hardware.
- HTTP daemon (`sandkilnd`) with full sandbox lifecycle: create, list,
  exec, stop.
- Per-sandbox networking: tap device pool, shared bridge, NAT, and a
  DNS proxy — verified with concurrent sandboxes reaching the internet.
- Cross-sandbox network isolation on the shared bridge (bridge port
  isolation) — verified: sandboxes can't reach each other, gateway and
  outbound internet still work.
- Bearer-token authentication on the daemon's API (`SANDKILN_AUTH_TOKEN`).
- Sandbox tags: set at creation, filterable via `?tag.<key>=<value>`.
- File read/write endpoints (`POST /sandboxes/:id/read-file`,
  `/write-file`), exposing the guest agent's existing file protocol
  through the HTTP API.
- Structured observability: `tracing` spans from the VM lifecycle layer
  up through HTTP request/response logging.
- JS/TS SDK (`sandkiln` package, first npm publish):
  `Sandbox.create()`/`attach()`/`list()` (tags, auth token), `runCommand()`,
  `readFile()`/`writeFile()`, `stop()`. ESM + CJS + full types — matches
  the daemon's entire HTTP surface, verified live against an auth-enabled
  daemon, published with signed provenance from the CI build.
- Python SDK (`sandkiln`, not published to PyPI yet): mirrors the JS SDK
  exactly, zero runtime dependencies (stdlib `urllib`). Verified live end
  to end, including `attach()` and correct 404 handling on a stopped
  sandbox.
- `kiln` CLI: `sandbox create|ls|rm|exec|read|write`, a thin wrapper over
  the SDK for manual/agentic use without writing code (published to npm
  in 0.2.0 — see above).
- `criterion` benchmarks (boot time, exec latency) and a scripted
  concurrent load-test script against the daemon's HTTP API.
- `scripts/integration-test.sh`: a full end-to-end test suite against a
  real running daemon (89 checks as of this release), covering every
  HTTP route this changelog lists.
- `AGENTS.md`: onboarding doc covering the non-obvious gotchas hit and
  fixed during development, so they don't get repeated.
- Project website (`website/`), deployed via GitHub Pages on every push.
- GitHub Actions: CI (build + clippy + SDK typecheck/build on every push)
  and a manual/tag-triggered npm publish workflow.
- `examples/code-playground` (JS/TS) and `examples/agent-runner` (Python)
  reference projects.

### Fixed
- Ambient `CAP_NET_ADMIN` not reaching Tokio's worker/blocking threads
  because `#[tokio::main]` starts the runtime before the capability was
  raised — restructured `main()` to raise it before entering the runtime.
- `DELETE /sandboxes/:id` returning `200` with an empty body instead of
  the documented `204`, which crashed the SDK's response parsing — found
  via live integration testing against a real daemon.
- Tap device creation via `ip tuntap add` failing under ambient
  `CAP_NET_ADMIN` (the `TUNSETIFF` ioctl needs real root) — switched to a
  pre-created persistent tap pool, leased/released via netlink calls
  only, which do work under ambient capability.
- A `pkill -f` pattern that could match its own invocation and kill the
  wrong process (including, once, the SSH session running it).
- A duplicate-command crash in the CLI (`program.command()` already
  registers a command; a trailing `addCommand()` re-added the same
  instance) — caught on the very first live run.
- Sandbox creation latency: rootfs clone now runs concurrently with the
  network lease instead of after it, and uses `cp --reflink=auto` (free
  copy-on-write where the filesystem supports it). Measured `create`
  latency mean dropped 369ms → 211ms.
- Python SDK import crash on Python <3.14 (`from __future__ import
  annotations` needed — see `AGENTS.md`), caught by CI on 3.12 even
  though local testing on 3.14 didn't hit it.
- CI build order: `kiln` needs `sandkiln`'s `dist/` built before it can
  typecheck, since it resolves it as a real workspace dependency.

## Known gaps (tracked in `ROADMAP.md`)

Current as of 0.3.0 — check there for anything that's landed since:

- No streamed exec output, no `kiln logs -f`.
- No true simultaneous parallel snapshot forking — at most one live fork
  of a given snapshot at a time (see the Persistence section).
- No OCI/Docker-image conversion for custom images — only an
  already-built ext4 rootfs file.
- WebSocket proxying (dev-server HMR/live-reload) through the preview
  proxy isn't implemented — plain HTTP only.
- Jailer's actual chroot/cgroup/uid-drop behavior hasn't been proven
  against a real installed jailer binary on real hardware yet — opt-in,
  not recommended as-is for adversarial workloads until verified.
- No per-sandbox seccomp filters, firewall/egress policy, or disk-size
  ceiling.
- On ext4 (no copy-on-write), sandbox creation still pays real rootfs
  copy time — needs a CoW-capable filesystem or a device-mapper layer to
  actually eliminate, not just overlap with other work.
- Python SDK not yet published to PyPI (code-side ready; needs the
  account owner's one-time trusted-publisher registration on pypi.org).
- Drives (attach at create, read-only sharing) aren't exposed in either
  SDK or the CLI yet — the raw daemon HTTP API only.
- Snapshot storage lives under `$TMPDIR` — durable across a daemon
  restart, not necessarily a host reboot (depends on whether `/tmp` is
  tmpfs on that host).

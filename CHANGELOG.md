# Changelog

Tracks notable changes across the whole project (daemon, core crates,
clients). The JS/TS SDK is published as [`sandkiln` on
npm](https://www.npmjs.com/package/sandkiln); everything else here
(daemon, core crates, Python SDK, CLI) doesn't have its own release yet.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/).

## Unreleased

**Published:** `sandkiln@0.2.0` (minor bump from 0.1.0 —
`resume`/`fork`/`snapshot`/`previewUrl` and `vcpuCount`/`memSizeMib`
overrides are all new, backward-compatible additions, nothing removed or
changed) and `sandkiln-cli@0.2.0` (first publish — `kiln`, the obvious
name, is a pre-existing unrelated package on npm, so the CLI publishes
under `sandkiln-cli` instead; `npm install -g sandkiln-cli` still gives
you the `kiln` command). Both on npm with signed provenance. Everything
below this line has landed in the repo since the SDK's 0.1.0 publish;
most of it is now live — check the published packages if you need to
know exactly what's on npm right now versus what's only in this repo.

### Added
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
- Core execution primitive: Firecracker microVM boot, a guest agent
  (vsock exec / read_file / write_file / list_dir), and a host-side
  vsock client, all proven end to end on real hardware.
- HTTP daemon (`sandkilnd`) with full sandbox lifecycle: create, list,
  exec, stop.
- Per-sandbox networking: tap device pool, shared bridge, NAT, and a
  DNS proxy — verified with concurrent sandboxes reaching the internet.
- Bearer-token authentication on the daemon's API (`SANDKILN_AUTH_TOKEN`).
- Sandbox tags: set at creation, filterable via `?tag.<key>=<value>`.
- File read/write endpoints (`POST /sandboxes/:id/read-file`,
  `/write-file`), exposing the guest agent's existing file protocol
  through the HTTP API.
- Structured observability: `tracing` spans from the VM lifecycle layer
  up through HTTP request/response logging.
- JS/TS SDK (`sandkiln` package, **published to npm, v0.1.0**):
  `Sandbox.create()`/`attach()`/`list()` (tags, auth token), `runCommand()`,
  `readFile()`/`writeFile()`, `stop()`. ESM + CJS + full types — matches
  the daemon's entire HTTP surface, verified live against an auth-enabled
  daemon, published with signed provenance from the CI build.
- Python SDK (`sandkiln` on PyPI, not yet published): mirrors the JS SDK
  exactly, zero runtime dependencies (stdlib `urllib`). Verified live end
  to end, including `attach()` and correct 404 handling on a stopped
  sandbox.
- `criterion` benchmarks (boot time, exec latency) and a scripted
  concurrent load-test script against the daemon's HTTP API.
- `AGENTS.md`: onboarding doc covering the non-obvious gotchas hit and
  fixed during development, so they don't get repeated.
- `kiln` CLI: `sandbox create|ls|rm|exec|read|write`, a thin wrapper over
  the SDK for manual/agentic use without writing code.
- Cross-sandbox network isolation on the shared bridge (bridge port
  isolation) — verified: sandboxes can't reach each other, gateway and
  outbound internet still work.
- Project website (`website/`), deployed via GitHub Pages on every push.
- GitHub Actions: CI (build + clippy + SDK typecheck/build on every push)
  and a manual/tag-triggered npm publish workflow.
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
- `scripts/preflight-check.sh` and `scripts/install-systemd-service.sh` +
  a real systemd unit template — a tested, reproducible self-hosting path,
  written up in full in `SELF_HOSTING.md`.
- `scripts/integration-test.sh`: a full end-to-end test suite against a
  real running daemon (89 checks as of this entry), covering every HTTP
  route this changelog lists.
- 144 Rust unit tests across the workspace, plus real unit tests for the
  CLI and both SDKs' pure logic (`node:test`/`unittest`).
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
- Snapshots living only in memory, so a daemon restart made every
  existing snapshot's files permanently unreachable through the API even
  though the bytes were still on disk — fixed by persisting metadata
  atomically and reconciling it at startup.
- Preview URLs 404ing on every SDK-generated URL: axum's `*path` wildcard
  doesn't match a bare trailing slash with nothing after it, which is
  exactly what `previewUrl()`'s default path produces — added the missing
  explicit route.

### Known gaps (tracked in `ROADMAP.md`)
- No custom/user-provided image support yet — only the universal base
  image.
- No streamed exec output, no `kiln logs -f`.
- No true simultaneous parallel snapshot forking — at most one live fork
  of a given snapshot at a time (see the Persistence section).
- WebSocket proxying (dev-server HMR/live-reload) through the preview
  proxy isn't implemented — plain HTTP only.
- Jailer's actual chroot/cgroup/uid-drop behavior hasn't been proven
  against a real installed jailer binary on real hardware yet — opt-in,
  not recommended as-is for adversarial workloads until verified.
- No per-sandbox seccomp filters or disk-size ceiling.
- On ext4 (no copy-on-write), sandbox creation still pays real rootfs
  copy time — needs a CoW-capable filesystem or a device-mapper layer to
  actually eliminate, not just overlap with other work.
- Python SDK not yet published to PyPI (code-side ready; needs the
  account owner's one-time trusted-publisher registration on pypi.org).
- Snapshot storage lives under `$TMPDIR` — durable across a daemon
  restart, not necessarily a host reboot (depends on whether `/tmp` is
  tmpfs on that host).

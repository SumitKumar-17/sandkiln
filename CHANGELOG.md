# Changelog

Notable changes across the project, newest first, loosely following
[Keep a Changelog](https://keepachangelog.com/). Package versions:
[`sandkiln`](https://www.npmjs.com/package/sandkiln) (JS/TS SDK),
[`sandkiln-cli`](https://www.npmjs.com/package/sandkiln-cli) (installs `kiln`),
and [`sandkiln` on PyPI](https://pypi.org/project/sandkiln/), versioned
independently. The daemon and core crates have no release of their own, so their
sections are dated. Current gaps and plans live in [`ROADMAP.md`](ROADMAP.md).

## Daemon and core, 2026-11-24

### Added
- Orderly shutdown: `SIGTERM`/`SIGINT` snapshots every live sandbox before exit.
  No leaked Firecracker processes; sandboxes are resumable after restart.
  `scripts/restart-test.sh` verifies it.
- `sandkiln.toml` config file below env vars, plus `sandkilnd --check-config`,
  `--print-config`, `--config-reference` and `sandkiln.example.toml`.
- `exec_timeout_secs` setting (default 600) and an integration topic for
  long-running commands.
- `retained_checkpoints_max` (default 8): the oldest time-travel checkpoints are
  evicted so ordinary resumes cannot fill the disk.
- Website: an Engineering section of posts with diagrams.

### Changed
- Daemon source split into `http/routes`, `lifecycle`, `snapshots` and `config`
  modules.
- `setup.sh` writes `sandkiln.toml`; `sandkilnd-ctl.sh` validates config before
  starting; `preflight-check.sh` reads the effective config and checks that the
  base image and `rootfs_dir` share a reflink filesystem.
- `scripts/remote.sh sync` prunes deleted files.

### Fixed
- A command silent for over 3s errored after about 6.5s and ran twice: the 3s
  control-plane timeout applied to user commands and the retry re-sent a
  delivered request. Retries now happen only if the request never reached the
  guest; user commands use `exec_timeout_secs` and return `504` on expiry.
- One slow command no longer freezes other sandbox operations (the sandbox lock
  was held across the guest call).
- Snapshot requests dropped from 6.8s to 0.56s: stopping the paused VM no longer
  waits on an unanswerable guest `sync`.
- Create unwinds the rootfs clone and network lease on every failure.
- Bearer-token comparison is constant time.

## Daemon and core, 2026-11-23

### Added
- `scripts/host-setup/create-reflink-volume.sh` and the
  `SANDKILN_INTEGRATION_TEST_ONLY` topic filter.

### Changed
- Rootfs clones use a native `FICLONE` reflink instead of `cp --reflink=auto` and
  report the method (`rootfs_clone_total{method}`). On XFS a cold create fell
  from about 187ms to 18ms.
- Snapshots and retired checkpoints moved from the OS temp dir to
  `SANDKILN_STATE_DIR` (default `~/sandkiln-tools/state`); rootfs copies to
  `SANDKILN_ROOTFS_DIR`. Snapshots left in the old location are not migrated.
- The guest DNS forwarder installs as a systemd service with `--bind-dynamic`,
  fixing a start-order failure and loss after reboot.

### Fixed
- RUSTSEC-2026-0459 (bundled SQLite) via `rusqlite` 0.43; `tokio` and `rustix`
  bumped past unsound-API advisories. `Cargo.lock` is committed.
- `scripts/remote.sh sync` no longer copies local credential files to the dev box.

## Daemon and core, 2026-10-07

### Added
- Local tunnel: the guest agent dials out to the host over a new `TUNNEL_PORT`
  vsock port, the one exception to "host always connects in".
  `Request::StartTunnel`/`StopTunnel`, `POST/DELETE /sandboxes/:id/tunnel` and a
  multiplexing `GET .../tunnel/:tunnel_id/ws`.

## Daemon and core, 2026-09-16

Everything since SDK/CLI release 0.7.0.

### Added
- Per-phase cold-create timings in `/metrics`
  (`create_phase_duration_ms{phase=...}`) and `scripts/dev-tools/profile-cold-create.sh`.
- Streamed background exec sessions (exec-stream) with replay then live tail.
- Remote storage mounts: `POST/GET/DELETE /sandboxes/:id/mounts` via
  `rclone mount` in the guest. Needs a `CONFIG_FUSE_FS` kernel and static
  `rclone` plus `fusermount3` in the rootfs.
- Time-travel restore: resume retires its checkpoint into `GET /snapshots/history`,
  restorable with `POST /snapshots/history/:id/restore`, reclaimable with `DELETE`.
- Snapshot lineage: `parent_snapshot_id` and a `?parent_snapshot_id=` filter.
- Per-sandbox egress policy (`egress: {mode, allow_cidrs, deny_cidrs}`), one
  iptables chain per sandbox, deny beats allow; survives snapshot/resume/fork.
- Archive tier: `SANDKILN_ARCHIVE_TIMEOUT_SECS` moves an unresumed snapshot's
  files to `SANDKILN_ARCHIVE_DIR`.
- Pool `max_count` with queueing (up to 30s) when a claim hits the ceiling;
  Python `Pool.create/list/delete`.
- Guest metadata service: named or tagged sandboxes serve `{id, name, tags}`
  through Firecracker MMDS.
- Durable history: `GET /sandboxes/history` backed by the new `sandkiln-store`
  sqlite crate.
- PTY sessions covered by the integration suite.
- Examples `snapshot-lifecycle` and `named-persistent-sandbox`.

### Changed
- Integration topics run in parallel (default 4, `SANDKILN_INTEGRATION_TEST_PARALLELISM=1`
  for sequential), each with its own workdir and counters.
- `egress::apply` uses one `iptables-restore` call: about 8.3ms to 3.1ms per create.
- `sandkiln-store` uses WAL with `synchronous=NORMAL`: 1897.8us to 37.1us per write.
- Profiling corrected two earlier conclusions: the rootfs clone is 74% of a cold
  create, not the network lease or Firecracker config calls.

### Fixed
- `POST /sandboxes` returning `200` did not mean the agent was listening; a cold
  first `exec` took 420-460ms. Retry loops now share `retry_with_backoff`, and a
  pooled sandbox pays 4-18ms instead.
- `wait_for_socket` used a fixed 20ms sleep per boot; now a connect-retry
  (`Vm::boot` 34.0ms to 11.3ms).
- `fork` shared its source's rootfs file; two sequential forks of one snapshot
  corrupted state. Each fork now gets a private copy.
- An orphaned shell after a PTY client disconnected.

## sandkiln (Python) 0.3.0, 2026-10-07

### Added
- `Sandbox.tunnel(guest_port, local_port)` and `AsyncSandbox.tunnel(...)`, with a
  hand-rolled RFC 6455 client to keep zero runtime dependencies.

## sandkiln (Python) 0.2.0, 2026-09-16

### Added
- `AsyncSandbox`, `AsyncDrive`, `AsyncImage`, `AsyncPool`: asyncio mirrors on a
  stdlib-based async HTTP client.

## sandkiln (Python) 0.1.0, 2026-09-15

First PyPI release, via OIDC trusted publishing. No code change.

## 0.12.0, 2026-10-07 (sandkiln, sandkiln-cli)

### Added
- Local tunnel: `Sandbox.tunnel(guestPort, { localPort })` and
  `kiln sandbox tunnel <id> <guest-port> --local-port <port>`;
  `examples/local-tunnel`.

## 0.11.0, 2026-09-16

### Added
- Egress policy in both SDKs and the CLI (`egress`, `egress_mode`/`egress_allow_cidrs`/
  `egress_deny_cidrs`, `--egress-mode`/`--allow-cidr`/`--deny-cidr`);
  `examples/egress-policy`.

## 0.10.0, 2026-09-16

### Added
- Remote storage mounts in both SDKs and the CLI (`mount`, `listMounts`,
  `unmount`; `kiln sandbox mount|mounts|unmount`).

## 0.9.0, 2026-09-16

### Added
- Environment variables: `env` on create (kept for the sandbox's lifetime) and
  per `exec`/`exec-stream` (merged on top, per-call wins); `--env key=value` in
  the CLI. Persists through snapshot/resume/fork.

## 0.8.0, 2026-09-16

### Added
- Streamed exec sessions: `POST/GET /sandboxes/:id/exec-stream` and a WebSocket
  logs route; `Sandbox.execStream()/listExecStreams()/attachLogs()`,
  `kiln sandbox exec-stream` and `kiln sandbox logs`.
- Pool `max_count`: claims past the ceiling queue, then return `503` after 30s.

### Changed
- `sandkiln-cli` depends on the matching `sandkiln` version and both publish together.

## 0.7.0, 2026-09-08

### Added
- Pre-warmed snapshot pools: `POST/GET /pools`, `DELETE /pools/:id`,
  `Pool.create/list/delete`, `kiln pool create|ls|rm`. A matching plain create
  transparently resumes a warm snapshot. A post-resume health check falls back to
  a cold create when a resumed guest is broken (a Firecracker/KVM resume failure
  seen on this dev box).

## 0.6.0, 2026-09-08

### Added
- Interactive terminals: `Sandbox.pty()` and `kiln sandbox pty <id>` over
  `GET /sandboxes/:id/pty`; `examples/interactive-terminal`.

### Fixed
- Disconnected PTY sessions left a side blocked or an orphaned shell; now closed
  with a socket shutdown and `SIGHUP`.

## 0.5.0, 2026-09-08

### Added
- Persistent drives in both SDKs and the CLI (`Drive.create/list/delete`, `drives`,
  `--drive <id[:ro]>`).
- Filesystem operations: `chmod`, `chown`, `mkdir`, `rename`, `copy`, `symlink`,
  `readlink`, `truncate` and `list_dir` with metadata, across protocol, daemon,
  both SDKs and the CLI.

### Changed
- `scripts/` reorganised: one-time provisioning moved to `scripts/host-setup/`,
  manual debugging tools to `scripts/dev-tools/`. Hosts that ran
  `allow-passwordless-cap-grant.sh` must re-run it from its new path.

## 0.4.0, 2026-09-08

### Added
- I/O rate limiting through Firecracker's token bucket: `rate_limit:
  {bandwidth_bytes_per_sec?, ops_per_sec?}` on create, applied to the rootfs,
  attached drives and both network directions. `0` or an empty object gets `400`.
  SDK `rateLimit`, CLI `--rate-bandwidth`/`--rate-ops`.

## 0.3.0, 2026-09-01

### Added
- Custom images: `POST/GET /images`, `DELETE /images/:id` (refused while in
  use), `image_id` on create, `Image.register/list/delete`, `kiln image ls|create|rm`.
  The daemon cannot verify the guest agent is baked in; use
  `preflight-check.sh --root-checks --rootfs-image`.
- Named sandboxes and persistent-by-default stop: `DELETE /sandboxes/:id`
  snapshots instead of destroying (`?keep=false` or `kiln sandbox rm --destroy`
  to destroy), `name` on create, `GET /sandboxes/by-name/:name`,
  `POST /sandboxes/get-or-create`.
- Auto-suspend (`SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`); `SANDKILN_IDLE_TIMEOUT_SECS`
  becomes the backstop and must be strictly larger. `GET /snapshots?source_sandbox_id=`.
- Read-only shared drives: attachable to any number of sandboxes at once;
  read-write still needs exclusivity. `attached_to` on drives is now a list of
  holders with a `read_only` flag (a raw HTTP API shape change).

### Fixed
- `get-or-create` without `name` returned `422` instead of `400`.
- Integration tests expecting the pre-persistent-stop `DELETE` behavior.

## 0.2.0, 2026-08-30

First publish of the CLI, as `sandkiln-cli` (`kiln` was taken on npm); the
command is still `kiln`.

### Added
- Persistent drives with cross-sandbox conflict detection.
- Snapshot/resume/fork, durable across daemon restart.
- Dev-server preview proxy (`/sandboxes/:id/preview/:port`), `previewUrl()`,
  `kiln sandbox preview`, `examples/dev-server-preview`.
- `vcpu_count`/`mem_size_mib` overrides with configurable ceilings.
- Idle-timeout reaper (`SANDKILN_IDLE_TIMEOUT_SECS`).
- Opt-in Firecracker jailer (`SANDKILN_JAILER_ENABLED`).
- `X-Request-Id` correlation, `/metrics`, `SANDKILN_LOG_FORMAT=json`, per-VM
  serial console log.
- `images/build-universal-image.sh`: Ubuntu, Node.js, Python, multi-agent users.
- `preflight-check.sh`, `sandkilnd-ctl.sh`, `install-systemd-service.sh` and a
  systemd unit template.
- Signed provenance on the CLI and SDK publish workflows.

### Fixed
- Preview URLs 404ing on a bare trailing slash (missing explicit route).
- Snapshots held only in memory; metadata is now persisted atomically and
  reconciled at startup.

## 0.1.0, 2026-08-26

First publish.

### Added
- Firecracker boot, guest agent (vsock exec, read, write, list) and host client.
- `sandkilnd` HTTP daemon: create, list, exec, stop; file read/write routes.
- Networking: tap pool, shared bridge, NAT, DNS proxy, and bridge port isolation
  between sandboxes.
- Bearer-token auth (`SANDKILN_AUTH_TOKEN`) and tags (`?tag.<key>=<value>`).
- `tracing` spans from the VM layer through HTTP logging.
- JS/TS SDK `sandkiln` (ESM and CJS, typed), a zero-dependency Python SDK
  (not yet on PyPI), and the `kiln` CLI (published in 0.2.0).
- `criterion` benchmarks, `scripts/load-test.sh`, and `scripts/integration-test.sh`.
- Project website (GitHub Pages), CI and npm publish workflows,
  `examples/code-playground` and `examples/agent-runner`.

### Fixed
- Ambient `CAP_NET_ADMIN` missing on Tokio worker threads: capability is now
  raised before the runtime starts.
- `DELETE /sandboxes/:id` returned `200` with an empty body instead of `204`.
- Tap creation under ambient `CAP_NET_ADMIN` (needs real root): replaced by a
  pre-created tap pool leased through netlink.
- A `pkill -f` pattern that could kill its own shell.
- A duplicate-command crash in the CLI.
- Create latency 369ms to 211ms: the rootfs clone runs concurrently with the
  network lease and uses reflink where available.
- Python SDK import crash on Python below 3.14 (missing `from __future__ import
  annotations`).
- CI build order: `kiln` needs `sandkiln`'s `dist/` first.

# Changelog

Not released to npm yet — this tracks notable changes to the project as a
whole (daemon, core crates, SDK) ahead of a first published version.
Format loosely follows [Keep a Changelog](https://keepachangelog.com/).

## Unreleased

### Added
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
- JS/TS SDK (`sandkiln` package): `Sandbox.create()`, `runCommand()`,
  `stop()`, `Sandbox.list()`. ESM + CJS + full types.
- `criterion` benchmarks (boot time, exec latency) and a scripted
  concurrent load-test script against the daemon's HTTP API.

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

### Known gaps (tracked in `ROADMAP.md`)
- No snapshot/resume or persistence — a stopped sandbox's state is gone.
- No image system beyond a fetched CI test rootfs — no universal base
  image, no custom image support yet.
- No isolation *between* sandboxes on the shared network bridge.
- No CLI, no Python SDK.
- Sandbox creation latency is dominated by a synchronous ~300MB rootfs
  copy, not VM boot — copy-on-write cloning is the next optimization.

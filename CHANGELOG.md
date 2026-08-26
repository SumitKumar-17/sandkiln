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
- JS/TS SDK (`sandkiln` package): `Sandbox.create()` (tags, auth token),
  `Sandbox.list()` (tag-filterable), `runCommand()`, `readFile()` /
  `writeFile()`, `stop()`. ESM + CJS + full types — matches the daemon's
  entire HTTP surface, verified live against an auth-enabled daemon.
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

### Known gaps (tracked in `ROADMAP.md`)
- No snapshot/resume or persistence — a stopped sandbox's state is gone.
- No image system beyond a fetched CI test rootfs — no universal base
  image, no custom image support yet.
- No Python SDK, no streamed exec output, no `kiln logs -f`.
- On ext4 (no copy-on-write), sandbox creation still pays real rootfs
  copy time — needs a CoW-capable filesystem or a device-mapper layer to
  actually eliminate, not just overlap with other work.
- npm publish is set up (workflow + package metadata) but not yet
  successfully run — needs an npm token type that actually bypasses 2FA
  for automated publishing.

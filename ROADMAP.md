# Roadmap

This is a working plan, not a spec. Phases will be reordered, merged, or
rewritten as we learn things — nothing here is fixed. Each phase should end
with something concretely runnable, not just code that compiles.

## Engineering principles

- **Modular, not monolithic.** Each concern is its own crate/package with a
  narrow public API (`vmm`, `guest-agent`, the daemon, etc. stay separable —
  no crate reaches into another's internals). A daemon change should not
  require touching the guest agent, and vice versa.
- **Benchmark the hot paths.** Boot time, exec round-trip latency, and
  snapshot/resume time are the metrics that actually matter for this
  product — each gets a `criterion` (Rust) or scripted benchmark once the
  path it measures exists, not bolted on at the end.
- **Prove it, don't assume it.** Every phase ends with something actually
  run on real hardware (the remote box), not just code that compiles.

Execution model: development happens in this repo; anything that needs KVM,
a Linux toolchain, or real hardware (Rust builds, Firecracker, rootfs/kernel
builds, actually booting a microVM) runs on the remote dev box over SSH.

## Phase 0 — Scaffolding
- Repo layout, license, gitignore, base tooling decisions.
- Rust workspace skeleton (`core/`), npm workspace skeleton (`packages/`).
- Confirm KVM access end-to-end on the remote box (device permissions, group
  membership, a `kvm-ok`-style check).

## Phase 1 — Prove the primitive: boot a microVM by hand
- Fetch/build a minimal guest kernel and a minimal rootfs.
- Get one Firecracker microVM booting via its API socket, driven by a small
  Rust binary — no daemon, no SDK yet, just "can we boot and reach a VM."
- Tap device + bridge + NAT so the guest has outbound network.

## Phase 2 — Guest agent + exec primitive
- Minimal static (musl) Rust agent baked into the rootfs, PID 1 or run from
  init, listening on a vsock port.
- Agent supports: exec a command and stream stdout/stderr/exit code, read
  file, write file, list directory.
- Host-side vsock client. End-to-end loop: boot → exec → get output →
  shut down.

## Phase 3 — Daemon and lifecycle API
- HTTP API server (axum) wrapping the VM lifecycle: create, exec (streamed),
  read/write files, list, stop, delete.
- Sandbox state tracking (in-memory first, then sqlite): id, status,
  resources, timestamps, tags.
- Per-sandbox resource limits (vCPU, memory), idle timeouts, orphan cleanup.

## Phase 4 — JS/TS SDK (`sandkiln`)
- `Sandbox.create()`, `sandbox.runCommand()`, `sandbox.readFile()` /
  `writeFiles()`, streamed logs, `sandbox.stop()`.
- Token-based auth against the daemon.
- Ship ESM + CJS + types.

## Phase 5 — CLI (`kiln`)
- `kiln sandbox create|exec|ls|rm|cp`, `kiln logs -f`.
- Built for manual testing, agentic workflows, and debugging — mirrors the
  SDK surface.

## Phase 6 — Persistence, snapshotting, images
- Snapshot/resume a stopped microVM (skip boot + dependency install).
- Managed base images (a small set of prebuilt rootfs images with common
  language runtimes) and custom image support.

## Phase 7 — Security hardening
- Firecracker jailer: chroot, cgroups v2 limits, seccomp, dropped
  capabilities, unprivileged per-VM uid.
- Network policy: isolated tap per sandbox, egress control, no
  sandbox-to-sandbox traffic by default.
- Multi-agent isolation within one sandbox (separate Linux users/home dirs,
  shared groups for controlled file sharing).

## Phase 8 — Dev servers and live preview
- Host-side reverse proxy that maps a sandbox port to a reachable URL, for
  previewing a dev server running inside a sandbox.
- Fast iterative file sync for dev-server-style workflows.

## Phase 9 — Drives and polish
- Attachable persistent storage that outlives a single sandbox and can be
  reattached to a new one.
- Tags (key/value metadata) for filtering and listing sandboxes.
- Structured logs and a `/metrics` endpoint.

## Phase 10 — Python SDK and docs
- `sandkiln` Python package mirroring the JS SDK.
- Docs site and runnable examples.

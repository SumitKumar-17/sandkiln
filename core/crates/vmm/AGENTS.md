# AGENTS.md — sandkiln-vmm

Read the root `AGENTS.md` first for project-wide conventions and the
gotchas list (several of them live in this exact crate's history — the
Tokio capability-ordering bug and the tuntap ioctl privilege issue both
happened here). This file is scoped to this one crate.

## What this crate is

The host-side library that actually drives Firecracker and networking.
`sandkiln-daemon` is a thin HTTP wrapper around this crate — if you're
implementing new VM-lifecycle behavior, it almost certainly belongs here,
not in `daemon`.

## Files

- `firecracker_api.rs` — a minimal hand-rolled HTTP/1.1 client for
  Firecracker's API Unix socket. Deliberately not a full HTTP client
  dependency — Firecracker's API surface is a handful of fixed-shape
  JSON PUT/PATCH requests, not worth pulling in `hyper` or similar for.
  If you need a new Firecracker API call, add a method here following
  the existing `put`/`patch` pattern.
- `vm.rs` — `Vm`/`VmConfig`: boot a microVM (spawn the Firecracker
  process, configure it over its API socket, start it), talk to its
  guest agent (`vm.call()`, retries briefly since the agent isn't
  listening the instant `InstanceStart` returns), stop it.
- `network.rs` — `NetworkManager`/`Lease`: the tap-device pool, bridge
  attachment, IP allocation, and bridge port isolation. Read the module
  doc comment at the top — it explains *why* a pool of pre-created tap
  devices exists instead of creating them on demand (ambient
  `CAP_NET_ADMIN` doesn't cover the `TUNSETIFF` ioctl, only netlink ops).
- `vsock_client.rs` — the host-side vsock connection, mediated through
  Firecracker's Unix-socket vsock bridging (`CONNECT <port>\n` handshake,
  then the connection is a raw byte stream to the guest agent).

## Building and testing

No KVM here means no real verification without the remote dev box — see
root `AGENTS.md`'s "Where the real work happens" section. `cargo build
-p sandkiln-vmm` / `cargo clippy -p sandkiln-vmm --all-targets` catch
compile errors, nothing more. The `examples/` directory
(`exec_test.rs`, `file_test.rs`) are real, runnable end-to-end checks
against a booted VM — look at them before writing a new one from scratch,
and prefer extending them over inventing a new ad hoc test harness.

Benchmarks live in `benches/vm_lifecycle.rs` (criterion) — see root
`ROADMAP.md`'s Benchmarking section for the exact env vars needed to run
them on the dev box.

## Non-obvious things specific to this crate

- **Ambient capabilities and Tokio don't mix the way you'd expect.** If
  code here ever needs to run inside a `tokio::task::spawn_blocking`
  closure AND needs a Linux capability, that capability has to be raised
  *before* the Tokio runtime starts (see root `AGENTS.md`) — this crate
  itself doesn't touch Tokio at all (it's synchronous, `daemon` wraps it
  in `spawn_blocking`), but if that ever changes, re-read that gotcha
  first.
- **`network.rs`'s tap pool is finite and shared.** `NetworkManager` has
  no visibility into which sandbox holds which lease beyond what the
  caller tracks — if you're adding a feature that needs to look up "which
  sandbox has this IP," that mapping needs to live in `daemon`'s
  `Sandbox` tracking, not here.
- Every privileged operation here (`ip`, `iptables`, `bridge` commands)
  runs via `std::process::Command`, not a native netlink library — this
  was a deliberate choice for simplicity and to match the shell scripts'
  behavior exactly, not an oversight. If you're tempted to switch to a
  crate like `rtnetlink`, make sure you understand why the current
  ambient-capability propagation works for spawned processes first (see
  the gotcha above) before assuming a library call would behave the same.

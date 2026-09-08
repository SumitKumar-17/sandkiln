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
- `vm/mod.rs` — `Vm`/`VmConfig`: the public API surface only —
  `Vm::boot`/`is_jailed`/`call`/`open_pty`/`update_metadata`/`stop`, plus
  the shared helpers both submodules below depend on
  (`console_log_path`/`console_log_stdio`/`annotate_with_console_log`/
  `wait_for_socket`/`path_str`/`put_checked`). The spawned process's
  stdout/stderr (the guest's `console=ttyS0` serial output) is captured
  to `/tmp/sandkiln-fc-<id>.log` rather than discarded —
  `annotate_with_console_log` appends that path to a boot failure's error
  message, since a guest kernel panic or agent crash before vsock comes
  up would otherwise be invisible from the host. This is exactly what
  caught a real, rare Firecracker/KVM bug while building `sandkiln-daemon`'s
  `pool` module: a resumed guest kernel occasionally panics early in boot
  (a divide-by-zero trap in the console driver, restored CPU/timer state
  interacting badly with timing-sensitive init code) and Firecracker
  exits shortly after — confirmed by reading exactly this console log,
  not guessed at. `snapshot::resume` uses the same shared helpers for the
  fresh Firecracker process it spawns.
  `update_metadata` redoes the full `PUT /mmds/config` + `PUT /mmds`
  sequence rather than a bare `PATCH /mmds`, for a real reason found
  live: a resumed VM's MMDS data store comes back **uninitialized** even
  though the VM had MMDS configured before being snapshotted — `PATCH
  /mmds` alone fails with "MMDS data store is not initialized" after a
  resume. This is the mechanism `sandkiln-daemon`'s pool-claim path uses
  to refresh a resumed sandbox's guest-visible identity away from
  whatever placeholder it had as a warm instance.
- `vm/boot.rs` — the actual boot mechanics, as a submodule of `vm` (not a
  sibling) specifically so it can still see `Vm`'s private fields to
  construct one. Split out of `vm/mod.rs` (2026-09-08, evidence-based
  crate audit) once that file mixed ~200 lines of boot-orchestration
  internals into what should be pure public API surface — same
  reasoning `vm/snapshot.rs` already gives for itself. `spawn_direct`/
  `spawn_jailed` resolve the process to spawn and the paths Firecracker's
  API calls should use (host paths for a direct boot, in-jail paths like
  `/kernel` for a jailed one — see `jailer.rs`); `configure_and_start`
  runs the same API PUT sequence either way, including inserting
  `rate_limit` (see `insert_rate_limiter`) into the drive/network-
  interface bodies when set. Also configures `VmConfig::metadata` (if
  set) via Firecracker's own MMDS — `PUT /mmds/config` then `PUT /mmds`,
  right after `/network-interfaces/eth0` (MMDS needs that interface
  already configured) and before `/vsock`/`InstanceStart`. This is a
  completely separate mechanism from everything else in this file — no
  vsock, no guest agent; Firecracker's device model answers
  `http://169.254.169.254/` requests from the guest directly. Errors
  loudly (`io::Error`) if `metadata` is set without `network` also being
  set, rather than silently doing nothing.
- `vm/snapshot.rs` — `Vm::pause`/`snapshot`/`resume` and `ResumeConfig`,
  as a submodule of `vm` (not a sibling) specifically so it can still see
  `Vm`'s private fields. Split out once `vm.rs` passed ~350 lines.
  `resume` always spawns directly — jailer support covers `Vm::boot`
  only; `daemon::routes_snapshot::snapshot_sandbox` refuses to snapshot a
  jailed sandbox rather than produce a snapshot `resume` can't correctly
  load (see `jailer.rs`'s module doc comment for why).
- `jailer.rs` — Firecracker's jailer: chroot, cgroup v2 limits, a
  dedicated uid/gid per VM. `JailerIdPool` allocates distinct uid/gid
  pairs (mirrors `network.rs`'s tap/IP pool); `link_resource_into_jail`
  places one host file inside a VM's chroot (hard link, falling back to a
  copy across filesystems) and reports the in-jail path Firecracker must
  use instead; `build_jailer_args`/`cgroup_limits` are pure and unit
  tested directly. Read its module doc comment before touching
  `vm/boot.rs`'s `spawn_jailed`/`spawn_jailed_inner` — the chroot
  path-rewriting is the part most likely to look right and be subtly
  wrong. **Not yet proven on real hardware**: enabling
  `SANDKILN_JAILER_ENABLED` breaks every sandbox create unless the
  `jailer` binary itself has been made setuid-root first (a separate,
  documented one-time step — see `SELF_HOSTING.md`'s "Optional:
  jailer-based sandbox boot") — confirmed live 2026-09-08 via the actual
  failure (`jailer` itself hits `Operation not permitted` trying to
  chown a hard-linked file into the chroot before that step is done).
- `network.rs` — `NetworkManager`/`Lease`: the tap-device pool, bridge
  attachment, IP allocation, and bridge port isolation. Read the module
  doc comment at the top — it explains *why* a pool of pre-created tap
  devices exists instead of creating them on demand (ambient
  `CAP_NET_ADMIN` doesn't cover the `TUNSETIFF` ioctl, only netlink ops).
- `vsock_client.rs` — the host-side vsock connection, mediated through
  Firecracker's Unix-socket vsock bridging (`CONNECT <port>\n` handshake,
  then the connection is a raw byte stream to the guest agent). Also
  `open_pty` — the same `CONNECT`-handshake dance against
  `sandkiln_protocol::PTY_PORT` instead of `AGENT_PORT`, followed by one
  framed `PtyHandshake` instead of a `Request`, after which the returned
  `UnixStream` is raw passthrough for its whole life (read/write timeouts
  are explicitly cleared before returning it, unlike every other call on
  this connection, since a PTY session is meant to sit idle between
  keystrokes). `Vm::open_pty` (in `vm/mod.rs`) wraps this the same way
  `Vm::call` wraps the request/response path — retrying for up to 5s
  while the guest agent's second listener comes up.

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
- **Jailer itself needs privileges the daemon deliberately doesn't have**
  (chroot, setuid/setgid, mknod for `/dev/kvm`/`/dev/net/tun` inside the
  jail, cgroup management) — this is why the `jailer` *binary* is made
  setuid-root as one-time setup (`SELF_HOSTING.md`), not something granted
  to the daemon via file capabilities the way `CAP_NET_ADMIN` is. Don't
  "fix" a jailer permission error by loosening the daemon's own
  capability set instead — see root `AGENTS.md` section 11.
- **`jailer.rs`'s tests use real temp directories and real hard
  links/`chmod`**, no KVM or jailer binary needed for any of them — same
  convention as `drive.rs`'s `TempStore`. What they can't cover without a
  real jailer binary and real chroot/cgroup/uid-drop behavior: whether
  the actual installed jailer version's directory ownership/permissions
  match what this module assumes. Verify that on the dev box before
  trusting jailer boot in production.

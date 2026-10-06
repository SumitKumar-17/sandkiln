# AGENTS.md — sandkiln-vmm

Read root `AGENTS.md` first — several of its gotchas (Tokio capability
ordering, the tuntap ioctl privilege issue) happened in this crate.

## What this crate is

The host-side library that drives Firecracker and networking.
`sandkiln-daemon` is a thin HTTP wrapper around it — new VM-lifecycle
behavior belongs here, not in `daemon`.

## Files

- **`firecracker_api.rs`** — hand-rolled HTTP/1.1 client for Firecracker's
  API Unix socket. Deliberately not a full HTTP dependency — a handful of
  fixed-shape JSON PUT/PATCH calls isn't worth `hyper` for. New
  Firecracker call → new method here, following the `put`/`patch`
  pattern.
- **`vm/mod.rs`** — `Vm`/`VmConfig` public surface
  (`boot`/`is_jailed`/`call`/`open_pty`/`open_exec_stream`/
  `update_metadata`/`stop`/`force_stop`) plus shared helpers both
  submodules use. Guest stdout/stderr is captured to
  `/tmp/sandkiln-fc-<id>.log`, not discarded — `annotate_with_console_log`
  appends that path to a boot failure, since a kernel panic before vsock
  comes up is otherwise invisible. This caught a real bug: a resumed
  guest kernel can panic early (divide-by-zero in the console driver,
  restored CPU/timer state vs. timing-sensitive init) — measured at
  roughly 1-in-3 to 2-in-3 resumes on this box, handled by a real
  post-resume health check (`daemon`'s `pool` module), not just
  documented. `update_metadata` redoes the full `PUT /mmds/config` + `PUT
  /mmds` rather than a bare `PATCH` — a resumed VM's MMDS store comes
  back uninitialized, and `PATCH` alone fails on it. `force_stop` skips
  `stop`'s "sync before kill" call, for a VM already known dead — without
  it, a failed health check paid two 5s timeouts back to back (~10.3s)
  instead of one (~5.4s).
- **`vm/boot.rs`** — boot mechanics, a submodule (not sibling) of `vm` so
  it can see `Vm`'s private fields. `spawn_direct`/`spawn_jailed` resolve
  the process and paths (host paths direct, in-jail paths like `/kernel`
  jailed — see `jailer.rs`); `configure_and_start` runs the same PUT
  sequence either way, built by `configuration_requests` as an ordered
  list so each PUT times individually (debug event `"vm boot phase
  breakdown"`). `InstanceStart` is timed separately — it's the one call
  that actually starts vCPUs, the rest only record config. Ordering
  constraint: MMDS after `/network-interfaces`. `insert_rate_limiter`
  wires `rate_limit` into drive/network bodies. MMDS setup
  (`VmConfig::metadata`) is a separate mechanism entirely — no vsock, no
  guest agent, Firecracker's device model answers
  `169.254.169.254` directly; errors loudly if `metadata` is set without
  `network`.
- **`vm/snapshot.rs`** — `Vm::pause`/`snapshot`/`resume`, `ResumeConfig`.
  `resume` always spawns directly — jailer covers `Vm::boot` only;
  `daemon::routes_snapshot::snapshot_sandbox` refuses to snapshot a
  jailed sandbox for this reason.
- **`jailer.rs`** — chroot, cgroup v2, a dedicated uid/gid per VM.
  `JailerIdPool` (mirrors `network.rs`'s tap/IP pool),
  `link_resource_into_jail` (hard link, falls back to copy across
  filesystems), `build_jailer_args`/`cgroup_limits` (pure, unit tested).
  **Not yet proven on real hardware** — `SANDKILN_JAILER_ENABLED` breaks
  every create unless `jailer` itself is made setuid-root first
  (`SELF_HOSTING.md`); confirmed live via `Operation not permitted`
  chown-ing a hard-linked file into the chroot before that step.
- **`network.rs`** — `NetworkManager`/`Lease`: tap pool, bridge
  attachment, IP allocation, port isolation. Pool exists (not on-demand
  creation) because ambient `CAP_NET_ADMIN` covers netlink, not the
  `TUNSETIFF` ioctl. `uplink()` exposes the uplink interface so
  `egress.rs` scopes rules to it the same way the bridge-wide `FORWARD`
  rule does. `attach_tap`'s three `ip`/`bridge` calls are each
  `fork`+`exec`, individually timed (~1.4ms each / ~4.3ms total) — not
  batched or moved to netlink, see `ROADMAP.md` Benchmarking.
- **`egress.rs`** — per-sandbox firewall: `EgressPolicy`
  (`AllowAll`/`DenyAll` + CIDRs), `apply()`/`remove()` managing one
  iptables chain per sandbox (`SK-EG-<tap_device>`) with a jump rule
  ahead of the bridge-wide `FORWARD` rule. Deny rules precede allow rules
  precede the mode default — first-match-wins makes deny always beat
  allow on overlap, no special-casing. Rules match only `-o <uplink>`
  traffic, so gateway-bound DNS never transits it and is structurally
  exempt. `apply()` uses one `iptables-restore --noflush` call instead of
  one spawn per rule (~8.3ms → ~3.1ms for a 6-CIDR policy, see
  `metrics::CreatePhase::EgressApply`); idempotent via a restore-format
  chain reset. Only the two `FORWARD`-touching rules stay individual
  `iptables` calls (`--noflush` can't touch `FORWARD` safely).
  `validate_cidr()` is string-format-only (`Ipv4Addr::from_str` + manual
  prefix bounds, no new dependency). Tied to the lease, not the VM — see
  `sandkiln-daemon/AGENTS.md` for call sites. IPv4 only.
- **`vsock_client.rs`** — host-side vsock via Firecracker's UDS bridging
  (`CONNECT <port>\n` handshake, then raw bytes). `open_pty` is the same
  handshake against `PTY_PORT`, then one framed `PtyHandshake`, then raw
  passthrough with read/write timeouts cleared (a PTY sits idle between
  keystrokes). `Vm::open_pty` wraps it with `retry_with_backoff` (1ms→20ms,
  up to 5s — the guest agent's startup race is slower/more variable than
  Firecracker's own API socket). This retry fix led to finding a cold
  sandbox's first real exec measures ~420-460ms end to end (resumed:
  ~4-18ms) — previously unmeasured. `open_exec_stream` is the same shape
  against `EXEC_STREAM_PORT` with an `ExecStreamHandshake`, reading
  framed `ExecStreamEvent`s instead of raw bytes.

## Building and testing

No KVM locally — `cargo build`/`clippy -p sandkiln-vmm` catch compile
errors only. `examples/` (`exec_test.rs`, `file_test.rs`) are real
end-to-end checks against a booted VM; extend them before writing a new
harness. Benchmarks: `benches/vm_lifecycle.rs` (criterion), env vars in
`ROADMAP.md`.

## Non-obvious things

- Raise capabilities before the Tokio runtime starts, not inside
  `spawn_blocking` — this crate itself is sync and doesn't touch Tokio,
  but re-read root `AGENTS.md` §12 if that changes.
- `NetworkManager` doesn't track which sandbox holds which lease —
  that mapping lives in `daemon`'s `Sandbox` tracking.
- Privileged ops (`ip`/`iptables`/`bridge`) run via
  `std::process::Command`, not a netlink library — deliberate, matches
  the shell scripts exactly. Don't switch to `rtnetlink` without
  understanding why ambient-capability propagation works for spawned
  processes but may not for a library call.
- `jailer` itself needs privileges the daemon deliberately doesn't have
  — that's why the `jailer` binary is setuid-root (one-time setup), not
  why the daemon's own capability set should be loosened.
- `jailer.rs`'s tests use real temp dirs/hard links/chmod, no KVM needed
  — they can't verify the actual installed jailer's directory
  ownership/permissions match what this module assumes. Verify that on
  the dev box before trusting jailer boot in production.

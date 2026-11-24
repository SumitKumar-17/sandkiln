# AGENTS.md: sandkiln-vmm

Read root `AGENTS.md` first. This is the host-side library that drives Firecracker
and networking. `sandkiln-daemon` is a thin HTTP wrapper around it: new VM-lifecycle
behavior belongs here, not there. The crate is synchronous and has no Tokio dependency.

## Files

- **`firecracker_api.rs`**: hand-rolled HTTP/1.1 client for Firecracker's API Unix socket (a few
  fixed-shape PUT/PATCH calls are not worth `hyper`). New Firecracker call, new method, following
  the `put`/`patch` pattern.
- **`vm/mod.rs`**: `Vm`/`VmConfig` public surface (`boot`, `is_jailed`, `call`, `open_pty`,
  `open_exec_stream`, `update_metadata`, `stop`, `force_stop`) and shared helpers. Guest console
  output goes to `/tmp/sandkiln-fc-<id>.log` and `annotate_with_console_log` appends that path to
  boot failures. `update_metadata` redoes `PUT /mmds/config` plus `PUT /mmds`, since a resumed VM's
  MMDS store is uninitialized and a bare `PATCH` fails. `force_stop` skips the pre-kill `sync` for a
  VM already known dead.
- **`vm/boot.rs`**: submodule of `vm` so it can see `Vm`'s private fields. `spawn_direct`/`spawn_jailed`
  resolve process and paths (host paths direct, in-jail paths like `/kernel` jailed).
  `configuration_requests` builds the ordered PUT list shared by both, each timed individually;
  `InstanceStart` is timed separately. MMDS must come after `/network-interfaces`, and `metadata`
  without `network` errors. `insert_rate_limiter` wires `rate_limit` into drive and network bodies.
- **`vm/snapshot.rs`**: `Vm::pause`/`snapshot`/`resume`, `ResumeConfig` (including `rootfs_override`).
  `resume` always spawns directly; the jailer covers `Vm::boot` only, so jailed sandboxes cannot be snapshotted.
- **`rootfs.rs`**: per-sandbox rootfs clone by native `FICLONE` reflink, falling back to a full copy,
  and reports which happened (`rootfs_clone_total{method}`).
- **`jailer.rs`**: chroot, cgroup v2, dedicated uid/gid per VM. `JailerIdPool`,
  `link_resource_into_jail` (hard link, copy across filesystems), pure `build_jailer_args` and
  `cgroup_limits`. Not yet proven on real hardware; the `jailer` binary must be setuid-root first
  (`docs-site/self-hosting/manual-setup.mdx`). Its tests use real temp dirs but cannot check a
  real installed jailer's ownership assumptions.
- **`network.rs`**: `NetworkManager`/`Lease`: tap pool, bridge attach, IP allocation, port
  isolation. A pool, not on-demand creation, because ambient `CAP_NET_ADMIN` covers netlink but
  not the `TUNSETIFF` ioctl. `uplink()` exposes the uplink so `egress.rs` scopes rules to it. It does
  not track which sandbox holds which lease; the daemon does.
- **`egress.rs`**: per-sandbox firewall. `EgressPolicy` (`AllowAll`/`DenyAll` plus CIDRs),
  `apply()`/`remove()` manage one iptables chain per sandbox (`SK-EG-<tap_device>`) with a jump
  ahead of the bridge-wide `FORWARD` rule. Order is deny, allow, mode default, so deny beats allow.
  Rules match only `-o <uplink>`, so gateway-bound DNS is exempt. `apply()` is one
  `iptables-restore --noflush` call; the two `FORWARD` rules stay individual calls. `validate_cidr()`
  is format-only. Tied to the lease, not the VM. IPv4 only.
- **`vsock_client.rs`**: host-side vsock over Firecracker's UDS bridge (`CONNECT <port>\n`, then raw
  bytes). `open_pty` and `open_exec_stream` do the same handshake against their ports and then frame
  differently; PTY clears read/write timeouts. `Vm::open_pty` and friends use `retry_with_backoff`
  (1ms to 20ms, up to 5s) because the guest agent's startup race is slow and variable.
- **`drive.rs`**: `DriveStore`, standalone ext4 files that outlive sandboxes. The filesystem is the
  source of truth (same pattern as `image.rs` and `sandkiln-store`).
- **`image.rs`**: `ImageStore` registers an existing ext4 rootfs (superblock check `looks_like_ext4`,
  then reflink copy into a managed dir). It cannot verify the guest agent (needs root), so responses
  carry `guest_agent_verified: false` and a `verification_hint`. See `docs-site/internals/images.mdx`.
- **`tunnel.rs`**: host-side acceptor for guest-initiated `TUNNEL_PORT` connections, the one exception
  to host-connects-in. `listen()` binds a `UnixListener` at the vsock UDS path plus `_<TUNNEL_PORT>`;
  each connection reads one `TunnelOpen` and is dispatched by `tunnel_id`. Uses `std::sync::mpsc` so the
  daemon bridges it through `spawn_blocking` like every other vmm call. `boot` and `resume` call
  `listen()` non-fatally; `stop_inner` calls `cleanup()`.

## Invariants

- Privileged ops (`ip`, `iptables`, `bridge`) run through `std::process::Command`, not a netlink
  library, because ambient capabilities propagate to spawned children. Do not switch to `rtnetlink`
  without checking that.
- The `jailer` binary is setuid-root so the daemon's own capability set stays minimal.
- A resumed guest can panic early (roughly 1 in 3 to 2 in 3 resumes on the dev box); the daemon's
  post-resume health check handles it, so do not assume a resume succeeded.

## Testing

No KVM locally. `cargo test -p sandkiln-vmm` covers pure logic (jailer args, egress rules, CIDR
validation). `examples/exec_test.rs` and `file_test.rs` are end-to-end checks against a booted VM on
the dev box; extend them before writing a new harness. Timing: `cargo bench -p sandkiln-vmm --bench
vm_lifecycle` (env vars in `ROADMAP.md`).

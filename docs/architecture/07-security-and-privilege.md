# Security and privilege model

## The problem

sandkiln's whole job is running code nobody vouched for. The core design
question isn't "how do we sandbox code" (Firecracker already gives a real
microVM: its own kernel, its own memory, hardware-virtualized) — it's "what
happens if a bug in *our own* code lets an attacker influence the daemon
itself." Every privilege decision below is about bounding that blast radius.

## The daemon never runs as root

`sandkilnd` runs as an ordinary user with exactly one Linux capability raised
into its **ambient set**: `CAP_NET_ADMIN`. Ambient capabilities are inherited
across `exec` without needing setuid-root, and are scoped to exactly the
syscalls that capability covers (netlink operations here) — not "root, but
named differently." See [03](03-networking-and-egress.md) for the specific
netlink-vs-`TUNSETIFF` boundary this bumps into.

**A real, hard-won ordering bug**: `setcap` grants don't survive a rebuild, and
`#[tokio::main]` starts the Tokio runtime — which spawns worker threads that
clone credentials at spawn time — *before* your function body runs. Raising a
capability into the ambient set has to happen in a plain `fn main()`, before
the runtime is entered, not inside a `#[tokio::main]`-wrapped body. Getting
this backwards was a long debugging session; `core/crates/daemon/src/main.rs`
now does it correctly and says why in a comment.

## Firecracker's jailer — one layer down

The VM process itself needs a few genuinely privileged one-time setup steps
(creating a chroot, setting cgroup limits, creating `/dev/kvm` and
`/dev/net/tun` device nodes inside the jail) that the always-unprivileged
daemon structurally cannot do itself. Firecracker's own `jailer` binary is the
standard setuid-root pattern for exactly this (the same shape as `passwd`
needing to write `/etc/shadow`): do the privileged setup, then drop every
privilege and `exec` the real unprivileged `firecracker` binary.

- **`chroot(2)`**: after this call, the process can't resolve any path outside
  the new root at all — not a permission check, the kernel simply won't do it.
  Every Firecracker API path (kernel image, rootfs, vsock socket) has to be
  rewritten to its in-jail path once this happens.
- **cgroup v2**: kernel-enforced caps on CPU/memory/process-count per VM.
- **Hard link, not copy**, to place a host file inside the jail
  (`link_resource_into_jail`) — instant, zero extra disk, falling back to a
  real copy only when jail and source are on different filesystems (hard links
  can't cross that boundary).
- **`JailerIdPool`** allocates a distinct uid/gid pair per VM from a pool —
  concurrently running jailed VMs never share one — the same pool-allocation
  pattern `sandkiln_vmm::network` already uses for tap devices/IPs.

## Where jailer stands today, honestly

Opt-in (`SANDKILN_JAILER_ENABLED`), off by default — every sandbox boots via a
direct Firecracker spawn unless explicitly turned on. Its own logic has unit
tests, but the one real-hardware attempt failed with `Operation not permitted`
because the `jailer` binary itself needs to be made setuid-root as a separate
manual step (`SELF_HOSTING.md`'s jailer section) that hadn't been done — an
**unmet setup precondition, caught live**, not a code bug, but a real reason
not to treat jailer mode as already proven without verifying it on your own
hardware first. Snapshotting a jailed sandbox also isn't supported yet
(`Vm::resume` always spawns directly).

## Boundary validation, at the server, not just the client

`drive::validate_id`/image `validate_id` reject path traversal
(`../etc/passwd`) before any filesystem call — the daemon does this itself
rather than trusting a client-side check, since a compromised or buggy client
is exactly the threat model here.

## Status

Direct-spawn boot: done, this is the default, live-verified path. Jailer: built,
unit-tested, **not yet proven end-to-end on real hardware**. See
[`website/src/content/docs/internals/jailer-privilege-model.md`](../../website/src/content/docs/internals/jailer-privilege-model.md)
for the real failure output from the one hardware attempt.

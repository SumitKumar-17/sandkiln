# Security hardening

## Isolation model

Every sandbox is a real Firecracker microVM with its own kernel, not a
shared-kernel container with syscall interception. That is the strongest
isolation available for untrusted code and the reason this project exists
in this shape. State it outright in docs and on the website.

## Status

| Item | State |
| --- | --- |
| Bridge port isolation (no sandbox-to-sandbox traffic) | Done, verified: cross-sandbox ping fails, gateway ping and outbound HTTP work |
| Per-sandbox resource ceilings | Done. `vcpu_count`/`mem_size_mib` overrides checked against `SANDKILN_MAX_VCPU_COUNT`/`SANDKILN_MAX_MEM_SIZE_MIB`; 0 or over the ceiling is a 400, never silently clamped; live-verified |
| Idle timeout | Done (`SANDKILN_IDLE_TIMEOUT_SECS`): a sandbox with no exec/read/write activity past the window is stopped automatically |
| I/O rate limiting | Done, below |
| Constant-time bearer comparison | Done (finding 15) |
| Jailer | Opt-in, built, not verified on real hardware, below |
| Encryption at rest | Filesystem-level only, below |
| seccomp filters, disk-size ceilings | Open |
| Runtime-configurable agent users | Open |
| GPU access | Not planned |

## Jailer (opt-in)

`SANDKILN_JAILER_ENABLED` gives chroot, cgroup v2 limits and a dedicated
unprivileged uid/gid per VM (`SELF_HOSTING.md`, "Optional: jailer-based
sandbox boot"). Off by default; the daemon spawns Firecracker directly. Builds and passes unit tests.
Snapshotting a jailed sandbox is unsupported (400): jailer support covers
`Vm::boot` only, and `Vm::resume` always spawns directly.

The first real-hardware attempt failed every `POST /sandboxes` with a 500
(`"<chroot>/root/api.sock" never appeared`); the console log showed jailer's
`Operation not permitted` chown, because the `jailer` binary was never made
setuid-root (a one-time step outside `setup.sh`, needing an interactive root
password, which that session lacked). Details are on the
[jailer internals page](../../docs-site/internals/jailer-privilege-model.mdx).
Still unverified on real hardware; verify before relying on it for an
adversarial workload.

## I/O rate limiting

`rate_limit: {bandwidth_bytes_per_sec?, ops_per_sec?}` on `POST /sandboxes`
and `get-or-create`. Mechanism (token buckets applied uniformly to the rootfs
and every drive and both NIC directions, full refill once per second,
validation that rejects rather than no-ops, a custom limit never matching a
pool): see the
[rate limiting internals page](../../docs-site/internals/rate-limiting.mdx).
Firecracker's independent burst and direction granularity stays available at
the vmm layer (`VmConfig::rate_limit`); `None` means unlimited. SDKs:
`rateLimit` / `rate_limit_bandwidth_bytes_per_sec` + `rate_limit_ops_per_sec`;
CLI: `--rate-bandwidth`/`--rate-ops`. The limiter fields were checked against
the real `firecracker_spec` swagger schema, not assumed.

## Encryption at rest

Scoped to filesystem level, not application level. Snapshot `mem.bin` (a
full RAM dump), drives, images and rootfs clones are plain files.
Firecracker opens and writes `mem.bin`/`state.snap` itself, so sandkiln
never holds the plaintext to encrypt; an application scheme would mean
decrypting before every Firecracker read and re-encrypting after every
write, add latency on hundreds of MB per snapshot, and need a key
management story (passphrase? external KMS?) this project does not have.
Recommendation (`SELF_HOSTING.md`, "Operational and security
considerations"): put snapshot, drive, image and archive storage on an
encrypted filesystem (LUKS or equivalent), which covers raw disk access.

## Multi-agent isolation

Done: `images/setup-multi-agent-users.sh` (run by
`images/build-universal-image.sh`) bakes in per-agent Linux users with
private `$HOME` and a shared group for deliberate sharing. Open: choosing
agent users per sandbox at create time.

## System-privileged workloads

Goal: workloads needing real guest privileges (container runtimes via
nested virtualization, VPN clients, FUSE drivers). The work is in the base
image (kernel config, cgroups inside the guest) more than in the daemon.
GPU passthrough is explicitly not planned: Firecracker has no GPU device
model, and adding one is a different project.

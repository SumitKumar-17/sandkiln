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
| Per-sandbox resource ceilings | Done. `vcpu_count`/`mem_size_mib` overrides checked against `SANDKILN_MAX_VCPU_COUNT`/`SANDKILN_MAX_MEM_SIZE_MIB`; 0 or over the ceiling is a 400, never silently clamped |
| Idle timeout | Done (`SANDKILN_IDLE_TIMEOUT_SECS`) |
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
sandbox boot"). Off by default; the daemon spawns Firecracker directly.
Snapshotting a jailed sandbox is unsupported (400): jailer support covers
`Vm::boot` only, and `Vm::resume` always spawns directly.

First real-hardware attempt failed: every `POST /sandboxes` returned 500
(`"<chroot>/root/api.sock" never appeared`). The console log showed jailer
failing with `Failed to change owner for .../firecracker: Operation not
permitted`. Not a code bug: the jailer binary needs `setuid-root`
(`sudo chown root:root` and `sudo chmod u+s`), a one-time step outside
`setup.sh` because it needs an interactive root password. It was never
applied on the box and the session had no interactive terminal. Still
unverified on real hardware; verify before relying on it for an adversarial
workload.

## I/O rate limiting

`rate_limit: {bandwidth_bytes_per_sec?, ops_per_sec?}` on `POST /sandboxes`
and `get-or-create`. Validation rejects rather than no-ops: at least one
sub-field if present, 0 is a 400. Applied uniformly to the rootfs drive,
every attached drive and both NIC directions through Firecracker's
token-bucket limiter (`rate_limiter`, `rx_rate_limiter`, `tx_rate_limiter`,
checked against the real swagger schema). Each bucket refills fully once per
second. A deliberately simple sandbox-level knob; Firecracker's independent
burst and direction granularity stays available at the vmm layer
(`VmConfig::rate_limit`). `None` means unlimited. SDKs: `rateLimit` /
`rate_limit_bandwidth_bytes_per_sec` + `rate_limit_ops_per_sec`; CLI:
`--rate-bandwidth`/`--rate-ops`.

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

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
| Jailer | Opt-in; works (finding 32 fixed): supervisor, shared jail group, group-owned taps, below |
| Encryption at rest | Filesystem-level only, below |
| seccomp filters, disk-size ceilings | Open |
| Runtime-configurable agent users | Open |
| GPU access | Not planned |

## Jailer (opt-in)

`SANDKILN_JAILER_ENABLED` gives chroot, cgroup v2 limits and a dedicated
unprivileged uid per VM, plus a shared jail group that the daemon belongs to.
Off by default; the daemon spawns Firecracker directly. Setup:
`docs-site/self-hosting/jailer.mdx`. Snapshotting a jailed sandbox is
unsupported (400): jailer support covers `Vm::boot` only, and `Vm::resume`
always spawns directly.

### History

The first real-hardware attempt failed every `POST /sandboxes` because the
`jailer` binary was never made setuid-root. Fixing that exposed finding 32:
jailer makes the chroot root mode 0700 and owned by the per-VM uid, so the
unprivileged daemon could not reach `api.sock`, and the failed boot's cleanup
(`kill` refused with `EPERM`, then an unbounded `wait`) hung the create. The
failure is now bounded and detected at startup, and the design below makes it
work.

### The problem, precisely

Four things have to work for an unprivileged daemon with a jailed Firecracker
running as another uid:

1. **Reach.** The daemon must connect to `api.sock` and `vsock.sock` inside
   the jail. Connecting to a UNIX socket needs write permission on the socket
   and search permission on every directory on the path
   ([unix(7)](https://man7.org/linux/man-pages/man7/unix.7.html)).
2. **Stop.** The daemon must end a Firecracker it cannot signal (`EPERM`
   across uids).
3. **Clean up.** It must delete the jail directory tree and the VM's cgroup,
   which jailer creates as root and never removes.
4. **Taps.** The jailed Firecracker must be allowed to attach to a pooled tap.

What jailer does is documented in the Firecracker jailer docs
(<https://github.com/firecracker-microvm/firecracker/blob/main/docs/jailer.md>):
it builds `<chroot_base>/<exec file name>/<id>/root`, copies the exec file in,
and chowns the chroot root to the uid and gid it was given "so that Firecracker
can create its API socket there"; it runs as root and `--netns`, `--new-pid-ns`
and `--parent-cgroup` are its only other knobs. Reading its source shows the
directories it creates in the chroot (`/`, `/dev`, `/dev/net`, `/run`) get an
explicit mode of 0700, set with `set_permissions` after the chroot, so nothing
done to the parent directories beforehand survives. It also insists the exec
file name contains "firecracker".

### Options evaluated

| Option | Verdict |
| --- | --- |
| Run the daemon as root, or give it `CAP_DAC_OVERRIDE` | Rejected. Defeats the unprivileged-daemon posture that the rest of the design depends on. |
| `CAP_DAC_READ_SEARCH` on the daemon | Rejected. It bypasses read and directory search checks, which is reach to the directory but not to the socket: connect needs write permission on the socket, which a process that only gets read and search overrides does not have. Fixing that needs another capability, and the capability applies to every file on the host. |
| `CAP_KILL` on the daemon | Rejected for stop and clean up: lets the daemon signal any process on the host. It would also leave reach unsolved. |
| ACLs or setgid on `--chroot-base-dir` so the jail root is traversable | Rejected. Jailer sets the root's mode explicitly with `chmod`, and `chmod` rewrites an ACL's mask from the group bits ([acl(5)](https://man7.org/linux/man-pages/man5/acl.5.html)), so a named-user ACL entry is masked away to nothing. Setgid inheritance does not help either: the mode 0700 still excludes the group. |
| `--uid`/`--gid` equal to the daemon's own | Works, trivially, and is rejected as the default: the jailed Firecracker would share an identity with the daemon, so a VMM escape could `ptrace` or signal the daemon, and per-sandbox uid isolation is lost. |
| A privileged helper that does the reaching, killing and deleting on the daemon's behalf | Rejected. A second root-privileged component with its own attack surface, to do three things that an ordinary process in the right group can do. |
| `--new-pid-ns`, `--netns` per VM | Orthogonal. Does not address reach, stop or taps. |
| Per-uid taps (tap `i` owned by uid `base + i`) | Not adopted. Stronger tap isolation, but couples the tap pool to the uid pool and would break direct-spawn resume (which runs as the daemon's user). See residual risks. |
| **A supervisor as jailer's exec file, plus one shared jail group** | **Adopted.** |

### The design

* **A shared jail group, `jailer_gid` (default 600000).** The daemon's user is
  a member. Every jailed VM runs under its own uid (from `JailerIdPool`) and
  this one gid, so isolation between sandboxes rests on distinct uids and
  distinct chroots, not on distinct groups.
* **`sandkiln-firecracker-supervisor`** (`core/crates/jail-supervisor`, a
  static musl binary of about 0.5 MB) is jailer's `--exec-file`. It starts as
  the VM's uid and the jail group, inside the chroot, after jailer has dropped
  privilege. It owns the jail root, so it changes the mode of `/`, `/dev`,
  `/dev/net` and `/run` to 2770 (group access, setgid so files the daemon
  creates inside belong to the jail group), sets umask 007 so the sockets
  Firecracker creates are group-accessible, then runs the real Firecracker,
  which the daemon copies in as `/firecracker`. An owner changing the mode of
  its own directory needs no privilege.
* **Stop through a pipe.** The daemon spawns jailer with a piped stdin and
  keeps the write end. When it closes (an explicit stop or the daemon dying)
  the supervisor SIGKILLs Firecracker and exits. This is the only thing that
  can stop a process the daemon may not signal, and it has a second benefit:
  a jailed VM can no longer outlive a crashed daemon. Verified: with a live
  jailed VM, `kill -9` of the daemon ends both processes within a second.
* **Clean up.** Because the directories are group-writable, the daemon
  removes the whole jail with `remove_dir_all`. The parent cgroup
  (`/sys/fs/cgroup/<supervisor name>`) is created once by
  `prepare-jailer.sh` and owned by the daemon's user; removing a cgroup needs
  write access to its parent, not to the cgroup itself, so the daemon removes
  each VM's cgroup after the process exits
  ([cgroup v2 delegation](https://docs.kernel.org/admin-guide/cgroup-v2.html)).
  At startup the daemon sweeps jails a previous process left (jail ids restart
  from 1 per process, so a stale one would collide).
* **Taps.** The pool is created with `ip tuntap add ... group <jail group>`
  and no owning user: the kernel checks a tap's owner and group, and a tap with
  an owner refuses a Firecracker running as any other uid. Group members may
  attach, which includes the daemon's own user for direct-spawn resume.
* **Writable resources** (the rootfs clone and drives) are given the jail
  group with group read-write instead of the previous world read-write, so a
  sandbox's disk is no longer writable by every host user.
* **Startup self-check** (`jailer_self_check`, default on) runs the real
  jailer once against a throwaway jail whose supervisor exits at once, then
  checks as the daemon's own user that it can use the jail root, remove it and
  remove the cgroup. It catches a jailer that is not setuid root, a missing
  cgroup delegation, a missing group membership and a non-static supervisor
  before any request is served.

Privilege inventory: the daemon gains nothing (still only ambient
`CAP_NET_ADMIN`). The jailer stays the only setuid-root binary. The new code
that runs inside the jail is the supervisor, as the VM's own unprivileged uid,
about a hundred lines of logic.

### Residual risks and limits

* **Shared gid.** A compromised jailed Firecracker has the jail group, which
  gives it group access to its own jail only (it cannot leave the chroot); but
  the tap pool is group-owned, so such a process could open another sandbox's
  tap by name and read or inject that sandbox's frames. Bridge port isolation
  still blocks ordinary sandbox-to-sandbox traffic; this is the case of an
  already compromised VMM. Per-uid taps would close it (the option above) at
  the cost described there.
* **Jail group membership is a privilege.** Any user in it can reach every
  jail's sockets. Put only the daemon's user in it.
* **The daemon's user owns the parent cgroup,** so it can create and remove
  child cgroups there; nothing else is delegated.
* **A crash leaves directories until the next start,** where the sweep
  removes them. A failure between jailer's chown and the supervisor's
  `chmod` (milliseconds) would leave a root-owned directory the sweep cannot
  remove; it is reported, with the path.
* **Snapshots of jailed sandboxes** stay unsupported.

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

# Self-hosting sandkiln

There is no hosted sandkiln service. Every instance is self-hosted: you
build the daemon, prepare a kernel and rootfs image, wire up host
networking, and run `sandkilnd` yourself. This guide is the complete,
tested path from a bare Linux host to a working daemon that will create
real, isolated microVM sandboxes.

Everything here corresponds to a real script, binary, or configuration
option that exists in this repository today — nothing is aspirational.
Where a step used to require several manual commands with paths that had
to match by hand, this guide also introduces the tooling
(`scripts/preflight-check.sh`, `scripts/install-systemd-service.sh`)
that now checks or automates it, rather than just describing the
workaround.

## 1. Requirements and prerequisites

- **Linux, x86_64.** Firecracker guests and the prebuilt kernel/rootfs
  tooling in this repo target x86_64 specifically; nothing here has been
  built or verified for aarch64.
- **`/dev/kvm`, readable and writable by the user who will run the
  daemon.** Bare metal or a VM with nested virtualization enabled both
  work — Firecracker only needs KVM, not bare metal specifically. Check
  with `ls -la /dev/kvm`; if your user isn't in the group that owns it,
  `sudo usermod -aG kvm $USER` and log in again.
- **Root/sudo for one-time host setup only.** The daemon itself never
  runs as root — see section 6 for exactly why and how.
- **Rust (stable, via `rustup`)** to build `core/`, plus the
  `x86_64-unknown-linux-musl` target and `musl-tools` package to build
  the guest agent (`rustup target add x86_64-unknown-linux-musl`; on
  Debian/Ubuntu, `sudo apt install musl-tools`).
- **`debootstrap`, `ubuntu-keyring`, and standard build tooling** if you
  intend to build a real production rootfs image rather than use the
  small test image (section 4 covers both). Debian/Ubuntu:
  `sudo apt install debootstrap ubuntu-keyring`.
- **Several GiB of free disk** if building a production image (the
  universal image defaults to 6GiB); the quick test image path needs
  under 500MiB.
- **Node.js 20+** only if you're building the CLI/SDK from source instead
  of installing the published `sandkiln`/`kiln` packages.

Run `scripts/preflight-check.sh` at any point to check where you actually
stand against everything below — it reads the same `SANDKILN_*`
environment variables and defaults the daemon itself uses, so it tells
you exactly what's missing before you try to start anything.

## 2. Installation and build

```
git clone https://github.com/SumitKumar-17/sandkiln.git
cd sandkiln/core
cargo build --release --workspace
```

This builds `sandkilnd` (the daemon, at `core/target/release/sandkilnd`)
and the `sandkiln-vmm`/`sandkiln-protocol` libraries it depends on. It
does **not** build the guest agent — that's cross-compiled separately for
the guest's musl target (section 4).

## 3. Firecracker and kernel

Install Firecracker and the jailer binary:

```
scripts/install-firecracker.sh ~/sandkiln-tools
```

This downloads both into `~/sandkiln-tools/bin`. Pin a specific version
with `FIRECRACKER_VERSION=v1.16.1 scripts/install-firecracker.sh ...` if
you need one other than the script's default.

You also need a guest kernel image. The quickest source is Firecracker's
own public CI artifacts:

```
images/fetch-test-image.sh ~/sandkiln-tools/images
```

This fetches a known-good kernel (`vmlinux-5.10.223`) **and** a small
rootfs image — the rootfs from this script is covered in the next
section as the "quick test" path, not the production one.

## 4. Base image and guest agent

A sandbox's rootfs needs the guest agent (`sandkiln-agent`) baked in and
running as a systemd service — without it, a sandbox boots but never
responds to `exec`/`read-file`/`write-file` (every call times out
retrying a vsock connection nothing is listening on). This applies
**regardless of which rootfs you use** and is easy to miss, since nothing
stops you from booting a sandbox from an image that's missing this step.

First, build the agent binary for the guest's target:

```
cargo build --release -p sandkiln-guest-agent --target x86_64-unknown-linux-musl
```

Then pick one of two rootfs paths:

### Quick test path

Use the small rootfs `images/fetch-test-image.sh` already downloaded
(section 3). It's ~300MiB and enough to prove the whole setup works end
to end, but it's missing `ca-certificates`, Node.js, Python, and most
common tooling — **do not use it to back real workloads.**

```
images/inject-agent.sh \
  core/target/x86_64-unknown-linux-musl/release/sandkiln-agent \
  ~/sandkiln-tools/images/ubuntu-22.04.ext4
```

(`inject-agent.sh` needs sudo — it loop-mounts the image.)

### Production path

Build a real rootfs from scratch — current Ubuntu LTS, current Node.js
LTS, Python 3, common CLI tooling, a working systemd init, and
[multi-agent isolation](#14-operational-and-security-considerations)
users, all in one image:

```
sudo images/build-universal-image.sh ~/sandkiln-tools/images/universal.ext4
```

Needs sudo (debootstrap, loop-mounting, chroot) and several GiB of free
disk during the build (6GiB by default — pass a size argument to change
it, see the script's header for every argument and env override it
accepts). This also runs `setup-multi-agent-users.sh` as its last step
automatically.

Then inject the agent into it, same as the quick path:

```
images/inject-agent.sh \
  core/target/x86_64-unknown-linux-musl/release/sandkiln-agent \
  ~/sandkiln-tools/images/universal.ext4
```

### Point the daemon at whichever image you built

**This step is easy to skip and the daemon will not tell you if you do:**
`SANDKILN_BASE_ROOTFS` defaults to
`~/sandkiln-tools/images/ubuntu-22.04.ext4` — the quick test image's
path, for local development convenience. If you built the production
image, you must set `SANDKILN_BASE_ROOTFS` to point at it explicitly
(section 7 covers all configuration). `scripts/preflight-check.sh` warns
if `SANDKILN_BASE_ROOTFS` looks like the small test image, and
`--root-checks` verifies the agent is actually baked into whatever image
is configured — run it before starting the daemon for real.

## 5. Networking and the TAP pool

The daemon manages its own bridge network at startup — you do not need
to create the bridge yourself. What you do need to create, once, as
root, is a pool of persistent TAP devices for the daemon to lease from:

```
sudo scripts/create-tap-pool.sh 32 $(whoami) sktap
```

This creates `sktap0`..`sktap31` owned by your user. `32` is your
concurrent-sandbox-with-networking ceiling for this host — raise it and
set `SANDKILN_TAP_POOL_SIZE` to match if you need more, and keep the two
in sync (`preflight-check.sh` checks this).

**Why a pre-created pool instead of creating TAP devices on demand:**
creating a *new* TAP device is a `TUNSETIFF` ioctl on `/dev/net/tun`,
which needs real root — unlike the netlink operations (attach/detach to
a bridge, bring up/down) the daemon performs at runtime under its
ambient `CAP_NET_ADMIN`, which do not need full root. Pre-creating the
pool once as root sidesteps this entirely: the daemon only ever
attaches/detaches *existing* devices from then on.

Guest DNS needs a forwarder, since guests can't necessarily reach public
resolvers directly but can reach the host's own resolver:

```
sudo scripts/start-dns-proxy.sh 172.16.0.1   # match SANDKILN_BRIDGE_GATEWAY
```

This isn't a systemd service yet — if you're running the daemon as a
persistent service (section 11), run this once per host boot too (a cron
`@reboot` entry or a small systemd unit of your own both work; not
shipped here yet, tracked as an open item in `ROADMAP.md`'s
Observability section).

`scripts/setup-tap-network.sh` is a **different, older tool** — a
point-to-point (single tap, no bridge) model used only by
`scripts/boot-test-vm.sh` for manual, outside-the-daemon boot testing.
It has nothing to do with the daemon's bridged-pool model above; don't
run it as part of a normal self-hosting setup.

## 6. Permissions and capabilities

The daemon needs `CAP_NET_ADMIN` to manage TAP attachment, the bridge,
and iptables rules — but runs as an **unprivileged user**, not root. This
is deliberate: running the whole HTTP-facing daemon as root would make
every bug in it a root-level bug; ambient `CAP_NET_ADMIN` gives it
exactly the one privilege it actually needs and nothing else.

There are two ways to grant it, and which one you want depends on how
you're running the daemon:

- **Manual (direct `./sandkilnd` invocation):**
  ```
  sudo scripts/grant-net-admin.sh core/target/release/sandkilnd
  ```
  This sets a file capability on the binary. **It does not survive a
  rebuild** — `cargo build --release` produces a new binary with no
  capability of its own, so this needs re-running after every rebuild or
  every network operation fails with "Operation not permitted."
- **Under systemd (recommended for anything persistent — section 11):**
  the provided unit grants `CAP_NET_ADMIN` via systemd's own
  `AmbientCapabilities=` directive at every service start, independent of
  whatever is or isn't `setcap`'d on the binary file. This means a
  rebuild needs no extra step at all under this path — verified directly
  as part of writing this guide: the binary's file capability was
  stripped entirely and the systemd-managed daemon still created,
  networked, and executed inside a real sandbox correctly.

Either way, the daemon's own startup code
(`raise_net_admin_ambient` in `core/crates/daemon/src/main.rs`) still
runs — it raises whatever's already in the process's Permitted set
(from either mechanism above) into the Inheritable and Ambient sets, so
child processes it shells out to (`ip`, `iptables`) inherit the
capability too.

## 7. Configuration

Every setting is an environment variable, all optional with the defaults
below (see `core/crates/daemon/src/config.rs` for the authoritative
source):

| Variable | Default | What it controls |
|---|---|---|
| `SANDKILN_LISTEN_ADDR` | `127.0.0.1:7777` | HTTP listen address |
| `SANDKILN_FIRECRACKER_BIN` | `~/sandkiln-tools/bin/firecracker` | path to the Firecracker binary |
| `SANDKILN_KERNEL_PATH` | `~/sandkiln-tools/images/vmlinux-5.10.223` | guest kernel image |
| `SANDKILN_BASE_ROOTFS` | `~/sandkiln-tools/images/ubuntu-22.04.ext4` | base rootfs cloned per sandbox — **the default is the small test image; override this for a production image, see section 4** |
| `SANDKILN_VCPU_COUNT` | `2` | vCPUs per sandbox |
| `SANDKILN_MEM_SIZE_MIB` | `512` | memory per sandbox |
| `SANDKILN_BRIDGE_NAME` | `sktapbr0` | bridge the daemon creates and manages |
| `SANDKILN_BRIDGE_GATEWAY` | `172.16.0.1` | bridge/gateway IP (a `/24` subnet) |
| `SANDKILN_UPLINK_IFACE` | auto-detected from the default route | host interface sandboxes NAT out through |
| `SANDKILN_TAP_POOL_PREFIX` | `sktap` | must match what `create-tap-pool.sh` was run with |
| `SANDKILN_TAP_POOL_SIZE` | `32` | must match what `create-tap-pool.sh` was run with |
| `SANDKILN_AUTH_TOKEN` | unset (auth disabled) | bearer token required on `/sandboxes*`, `/drives*`, `/snapshots*` — **unset means the API is completely open, see section 9** |
| `SANDKILN_DRIVES_DIR` | `~/sandkiln-tools/drives` | where persistent drives are stored |
| `SANDKILN_IDLE_TIMEOUT_SECS` | unset (disabled) | auto-stop a sandbox after this many idle seconds (no exec/read/write activity); `0` also disables it |
| `SANDKILN_LOG_FORMAT` | `pretty` | set to `json` for one JSON object per log line, for log pipelines that parse fields |

Paths accept a leading `~/` and are expanded against `$HOME` at startup —
useful when scripting, but be aware this means `$HOME` must be set
correctly for whichever user actually runs the process (see the `sudo
-E` note in section 10 for where this has bitten before).

## 8. Starting the daemon

Once sections 3–7 are done, validate everything before starting for
real:

```
scripts/preflight-check.sh --daemon-bin core/target/release/sandkilnd
```

Fix anything reported as `FAIL`; review anything reported as `WARNING`.
Then start it:

```
SANDKILN_AUTH_TOKEN=$(openssl rand -hex 32) \
  core/target/release/sandkilnd
```

Or, for a persistent, supervised deployment, skip straight to section 11
instead of running it in a foreground shell.

## 9. Authentication and security

**Set `SANDKILN_AUTH_TOKEN` before exposing the daemon beyond
localhost.** With it unset, every route except `/healthz` and `/metrics`
is completely open — anyone who can reach the port can create sandboxes,
read/write files inside them, and delete persistent drives. The daemon
logs a startup warning when this is the case specifically so it's never
silent. When set, every `/sandboxes*`, `/drives*`, and `/snapshots*`
request needs `Authorization: Bearer <token>` matching it exactly
(`core/crates/daemon/src/auth.rs`); a mismatched, missing, or malformed
header gets `401`.

This is a single shared-secret token, not a multi-tenant identity
system — this is a self-hosted, single-operator daemon, not a platform.
If you need per-caller scoping (which sandboxes a given token can see),
that isn't implemented yet — see `ROADMAP.md`'s Authentication section.

Beyond the token, see section 14 for the broader security posture
(privilege model, network isolation between sandboxes, what's still
open).

## 10. Verification with a real sandbox

Two ways to actually prove the daemon works, from simplest to most
thorough:

**Manual, by hand:**
```
curl http://127.0.0.1:7777/healthz
curl -X POST http://127.0.0.1:7777/sandboxes \
  -H "Authorization: Bearer $SANDKILN_AUTH_TOKEN"
# {"id": "..."}
curl -X POST http://127.0.0.1:7777/sandboxes/<id>/exec \
  -H "Authorization: Bearer $SANDKILN_AUTH_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"command": "echo", "args": ["hello from a real microVM"]}'
curl -X DELETE http://127.0.0.1:7777/sandboxes/<id> \
  -H "Authorization: Bearer $SANDKILN_AUTH_TOKEN"
```

**Automated, full coverage:**
```
SANDKILN_AUTH_TOKEN=<your-token> scripts/integration-test.sh
```
This exercises the complete API end to end — sandbox lifecycle, tags,
drives (including persistence across sandboxes and conflict detection),
snapshot/resume, auth, `/metrics`, and error cases — and cleans up
everything it creates whether it passes or fails. This is exactly the
process used to validate this guide itself; see `AGENTS.md`'s
"Integration testing" section for more.

Or point a real client at it instead of `curl`:
```
npm install -g kiln
kiln sandbox create --base-url http://127.0.0.1:7777 --token $SANDKILN_AUTH_TOKEN
```
```
pip install sandkiln
```
Each SDK/CLI's own `AGENTS.md` documents its exact configuration surface
(`packages/sdk`, `packages/python`, `packages/cli`) — all three default to
`SANDKILN_DAEMON_URL`/`SANDKILN_AUTH_TOKEN` env vars matching the
daemon's own naming.

## 11. Running as a persistent service

```
sudo scripts/install-systemd-service.sh \
  $(pwd)/core/target/release/sandkilnd <run-as-user> /etc/sandkiln/sandkilnd.env
```

This installs a real systemd unit (from `scripts/sandkilnd.service.template`)
that:

- Runs `sandkilnd` as the unprivileged user you specify, not root.
- Grants `CAP_NET_ADMIN` via `AmbientCapabilities=` (section 6) — no
  `setcap` step, and no re-granting after a rebuild.
- Runs `scripts/preflight-check.sh` automatically before every start
  (`ExecStartPre`) — a bad config or missing prerequisite fails the
  start with a clear reason in the journal instead of a daemon that
  starts and then fails every request.
- Restarts automatically on failure.
- Reads its `SANDKILN_*` configuration from the env file you point it
  at, created with a template on first install (edit it — at minimum set
  `SANDKILN_AUTH_TOKEN` before enabling this beyond localhost).

Then:
```
sudo systemctl enable --now sandkilnd
journalctl -u sandkilnd -f
```

Verified directly while writing this guide: a full `sandkilnd` lifecycle
(install, start, create/exec/stop a sandbox, `systemctl restart`, full
`scripts/integration-test.sh` run — 43/43 passing) with the binary's file
capability stripped entirely, proving the `AmbientCapabilities=` path
works independent of `setcap`.

The DNS proxy (section 5) isn't wired into this unit yet — start
`scripts/start-dns-proxy.sh` separately (or via your own small unit/cron
`@reboot` entry) if sandboxes need outbound DNS on a host that reboots.

## 12. Upgrades, rebuilds, and what must be repeated

| You changed... | You must also... |
|---|---|
| daemon/vmm/protocol Rust source | `cargo build --release --workspace`. Under the systemd unit: nothing else. Running manually: re-run `scripts/grant-net-admin.sh` (section 6). |
| the guest agent (`core/crates/guest-agent`) | rebuild it for the musl target, then **re-inject it into every rootfs image currently in use** (`images/inject-agent.sh`) — a rebuilt agent binary sitting on the host does nothing until it's baked into the image `SANDKILN_BASE_ROOTFS` actually points at. |
| the base image / added packages to it | rebuild it (`images/build-universal-image.sh`), re-inject the agent, and repoint `SANDKILN_BASE_ROOTFS` if you built it under a new path. Sandboxes already running keep using whatever image they booted from; only new sandboxes pick up the change. |
| `SANDKILN_TAP_POOL_SIZE` upward | re-run `scripts/create-tap-pool.sh` with the new count (it's idempotent — existing devices are left alone, only missing ones are created) before raising the env var, or the daemon's own startup `ensure_ready()` check fails loudly rather than silently under-provisioning. |
| Firecracker itself | re-run `scripts/install-firecracker.sh`, optionally pinning `FIRECRACKER_VERSION`. |

Run `scripts/preflight-check.sh` again after any of the above before
trusting the result — that's exactly what it exists for.

## 13. Troubleshooting

- **"Operation not permitted" on any network call** — you rebuilt the
  daemon and are running it manually (not under the systemd unit) and
  forgot to re-run `grant-net-admin.sh`. See section 6.
- **`tap devices missing: [...] — run scripts/create-tap-pool.sh first`**
  at startup — `SANDKILN_TAP_POOL_PREFIX`/`SANDKILN_TAP_POOL_SIZE` don't
  match what you actually created, or the pool was created for a
  different user than the daemon runs as.
- **Sandboxes create fine but `exec`/`read-file`/`write-file` time out
  or fail with a vsock connect error** — the guest agent isn't baked
  into whatever rootfs `SANDKILN_BASE_ROOTFS` points at. Run
  `sudo -E scripts/preflight-check.sh --root-checks` to confirm, then
  see section 4.
- **Sandboxes boot but have no outbound network** — confirm
  `SANDKILN_UPLINK_IFACE` detected the right interface (`ip route show
  default`), and that `scripts/start-dns-proxy.sh` is actually running
  for guest DNS specifically to work (outbound IP connectivity and DNS
  resolution are independent failure modes — check both).
- **`/dev/kvm: permission denied`** — `sudo usermod -aG kvm $USER`, then
  log in again (group membership doesn't apply to an already-open
  session).
- **`scripts/preflight-check.sh --root-checks` reports missing binaries
  that you know are installed** — you ran it with plain `sudo`, which
  resets `$HOME` to `/root` and breaks every `~/sandkiln-tools/...`
  default path. Use `sudo -E` to preserve your environment instead.
- **A rebuilt production image's debootstrap step fails on an unknown
  codename** — your host's `debootstrap` doesn't yet know the Ubuntu
  release `build-universal-image.sh` defaults to; either
  `apt-get install --only-upgrade debootstrap` or pass an older LTS
  codename explicitly as its third argument (its own header comment has
  the exact command).
- **Disk fills up during a production image build** — it needs several
  GiB free for the duration of the build (the sparse image itself, plus
  package downloads inside the chroot); free space first rather than
  letting it fail partway (the script cleans up a partial image on
  failure, but doesn't reduce how much headroom the next attempt needs).

## 14. Operational and security considerations

- **Privilege model**: the daemon runs unprivileged with only ambient
  `CAP_NET_ADMIN` (section 6) — never as root. Tap device *creation*
  needs real root and is deliberately a one-time, out-of-band step
  (`create-tap-pool.sh`), not something the running daemon ever does
  itself, keeping its own privilege footprint minimal.
- **Network isolation between sandboxes**: every sandbox's tap device has
  bridge port isolation enabled — a sandbox can reach the gateway/uplink
  (for outbound internet) but not another sandbox's tap on the same
  bridge. Verified: cross-sandbox traffic fails, gateway and real
  outbound traffic both still work.
- **Authentication is all-or-nothing today** (section 9) — one shared
  token, no per-caller scoping yet. Treat any token holder as fully
  trusted with everything the daemon can do.
- **Isolation inside a sandbox, not just between them**: the production
  image bakes in [multi-agent isolation](ROADMAP.md) — separate
  `agentN` Linux users with private (mode 700) home directories and a
  deliberate, opt-in `/srv/shared` for cross-agent file sharing within
  one sandbox. This is about multiple cooperating processes inside one
  VM, not a substitute for the VM-level isolation between sandboxes
  above.
- **Firecracker's jailer** (chroot, cgroups v2 resource limits, seccomp,
  dropped capabilities, an unprivileged uid per VM) is not wired into the
  daemon's boot path as of this writing — every sandbox boots via a
  direct Firecracker process spawn, with isolation coming from
  KVM/hardware virtualization itself plus the network controls above,
  not from jailer hardening on top. Check `ROADMAP.md`'s Security
  hardening section for current status before relying on this for a
  genuinely adversarial multi-tenant workload.
- **No per-sandbox resource ceiling enforcement yet** — `vcpu_count`/
  `mem_size_mib` apply uniformly to every sandbox via daemon config;
  nothing today stops a caller from running a sandbox indefinitely
  except the optional `SANDKILN_IDLE_TIMEOUT_SECS`.
- **Persistent state and where it lives**: persistent drives live under
  `SANDKILN_DRIVES_DIR`. Snapshots (state + memory + metadata) live under
  `$TMPDIR/sandkiln-snapshots` (`std::env::temp_dir()` joined with a
  fixed subdirectory — typically `/tmp/sandkiln-snapshots`;
  `core/crates/daemon/src/snapshot.rs::snapshots_root()` is the
  authoritative source). Both are the only state that outlives a sandbox
  stop — back them up if they matter to you. Snapshots specifically are
  durable across a **daemon restart** (verified live: the daemon
  reconciles every valid snapshot directory back into memory at
  startup — killed a daemon with `kill -9` mid-session with a snapshot on
  disk, started a fresh instance, and resumed it successfully with its
  data intact) but **not necessarily across a host reboot** — whether
  `/tmp` survives a reboot depends on that host's own configuration (many
  systemd-based distributions mount `/tmp` as tmpfs by default, which
  does not survive a reboot). If snapshot durability across a reboot
  matters for your deployment, point `TMPDIR` at a path on persistent
  storage before starting the daemon, or move `SANDKILN_DRIVES_DIR` and
  snapshot storage onto the same durable volume and verify your host's
  `/tmp` mount before relying on the default. The daemon's own
  bookkeeping of *running* sandboxes is in-memory only regardless — a
  restart never preserves live sandboxes (their VM processes exit with
  the daemon), only drives and snapshots on disk.
- **Cleanup on crash**: if `sandkilnd` is killed ungracefully, any
  Firecracker child processes it had spawned are orphaned (not part of
  a process group the daemon tears down on its own exit) — check
  `pgrep -a firecracker` and `pkill -x firecracker` (exact-name match,
  not `-f`, to avoid matching an unrelated process whose command line
  happens to contain the string) if sandboxes seem stuck after an
  unclean daemon exit, then restart the daemon.

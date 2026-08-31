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
(`scripts/setup.sh`, `scripts/preflight-check.sh`,
`scripts/sandkilnd-ctl.sh`, `scripts/install-systemd-service.sh`) that
now checks or automates it, rather than just describing the workaround.

## Quick start

Sections 1–8 below, condensed to two commands, for a host that already
has Rust and Firecracker's own prerequisites (KVM, sudo) available:

```
scripts/setup.sh          # builds everything, fetches a test image,
                           # injects the guest agent, creates the tap
                           # pool, grants CAP_NET_ADMIN — idempotent,
                           # safe to re-run
scripts/sandkilnd-ctl.sh start
```

That's a real, working daemon — enough to prove the whole stack end to
end — but booting from the small Firecracker CI test image (~300MiB,
missing `ca-certificates` and any language runtime). Add `--production`
to `setup.sh` for a real base image (needs sudo and ~8GiB free disk;
takes several minutes): `scripts/setup.sh --production`. Read sections
1–8 if you want to understand what each step is actually doing, need to
customize something the flags don't cover, or are debugging a step that
failed.

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

### Optional: jailer-based sandbox boot

By default sandboxes boot via a direct Firecracker process spawn — no
chroot, no cgroup limits, no per-VM uid separation beyond KVM itself and
the daemon's own unprivileged-with-ambient-`CAP_NET_ADMIN` posture.
Firecracker ships a companion binary, `jailer` (already downloaded into
`~/sandkiln-tools/bin` by step 2), that re-execs Firecracker inside a
chroot'd, cgroup-v2-limited environment running as a dedicated
unprivileged uid/gid instead. This is opt-in, off by default — see
`ROADMAP.md`'s "Security hardening" section for why it's worth turning on
for anything running genuinely untrusted workloads.

**cgroups v2 must be enabled on the host** (the default on any current
distro; check with `mount | grep cgroup2` — a `cgroup2` mount at
`/sys/fs/cgroup` means it's ready). Jailer is invoked with
`--cgroup-version 2` unconditionally; a host still on the legacy cgroups
v1 hierarchy needs to switch first (out of scope here — it's a kernel
boot parameter, not something this project's tooling manages).

**The `jailer` binary itself needs to run with privileges the daemon
deliberately doesn't have** — chroot(2), setuid/setgid to drop to the
per-VM uid/gid, creating device nodes for `/dev/kvm`/`/dev/net/tun` inside
the jail, and cgroup management. Rather than grant the *daemon* those
capabilities (which would be most of what root can do anyway, defeating
the whole point of the unprivileged-daemon posture described below in
"Why not just run as root"), make the small, purpose-built `jailer`
binary itself setuid-root — the standard way Firecracker's own jailer is
deployed without running its caller as root:

```
sudo chown root:root ~/sandkiln-tools/bin/jailer
sudo chmod u+s ~/sandkiln-tools/bin/jailer
```

This does **not** survive a re-run of `scripts/install-firecracker.sh`
(it overwrites the binary) — re-apply it after any jailer upgrade, the
same way `grant-net-admin.sh` has to be re-run after every daemon
rebuild.

Then set:

```
SANDKILN_JAILER_ENABLED=true
```

Everything else has a default (see the table below): `SANDKILN_JAILER_BIN`
(`~/sandkiln-tools/bin/jailer`), `SANDKILN_JAILER_CHROOT_BASE_DIR`
(`~/sandkiln-tools/jail` — created automatically at startup if missing;
keep it on the **same filesystem** as wherever `SANDKILN_DRIVES_DIR` and
the OS temp dir live, so the rootfs/drive files this hard-links into each
VM's chroot link instantly instead of falling back to a real copy on
every boot), `SANDKILN_JAILER_UID_GID_BASE` (`600000`) and
`SANDKILN_JAILER_POOL_SIZE` (`32`) — together these reserve
`600000..600031` as a dedicated uid/gid range, handed out one distinct
pair per concurrently-running jailed VM and released back on stop. Pick a
base outside any real host account's uid (the `/etc/subuid` convention of
staying at or above `100000`, and clear of anything `/etc/passwd` already
uses, is the right instinct) — two jailed VMs ever sharing a uid would let
one guest's escaped process interfere with the other's, which defeats the
entire point.

Jailer support is a daemon-operator setting, not something an individual
`POST /sandboxes` request can opt out of — that's deliberate, so a
compromised or malicious API client can't disable a security boundary the
operator turned on.

**Known limitation:** snapshotting a jailed sandbox isn't supported yet —
`POST /sandboxes/<id>/snapshot` returns `400` for one. Resuming a snapshot
always uses a direct spawn regardless of whether jailer is enabled. See
`core/crates/vmm/src/jailer.rs`'s module doc comment for why.

**Recommendation as of this writing:** keep this opt-in rather than
default. It builds and passes every unit test that doesn't need a real
jailer binary, but the chroot/cgroup/uid-drop behavior against a real
installed jailer hasn't been proven on real hardware yet — verify it
there (boot a jailed sandbox, confirm `ps`/`ls` on the host shows the
Firecracker process running as the dedicated uid inside its chroot, run
`scripts/integration-test.sh` against a jailer-enabled daemon) before
relying on it for a genuinely adversarial multi-tenant workload.

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
| `SANDKILN_VCPU_COUNT` | `2` | vCPUs per sandbox, when a request doesn't override it |
| `SANDKILN_MEM_SIZE_MIB` | `512` | memory per sandbox, when a request doesn't override it |
| `SANDKILN_MAX_VCPU_COUNT` | `16` | ceiling on a per-request `vcpu_count` override (`POST /sandboxes`); a request above this, or `0`, is rejected with `400` |
| `SANDKILN_MAX_MEM_SIZE_MIB` | `16384` | ceiling on a per-request `mem_size_mib` override, same semantics as above |
| `SANDKILN_BRIDGE_NAME` | `sktapbr0` | bridge the daemon creates and manages |
| `SANDKILN_BRIDGE_GATEWAY` | `172.16.0.1` | bridge/gateway IP (a `/24` subnet) |
| `SANDKILN_UPLINK_IFACE` | auto-detected from the default route | host interface sandboxes NAT out through |
| `SANDKILN_TAP_POOL_PREFIX` | `sktap` | must match what `create-tap-pool.sh` was run with |
| `SANDKILN_TAP_POOL_SIZE` | `32` | must match what `create-tap-pool.sh` was run with |
| `SANDKILN_AUTH_TOKEN` | unset (auth disabled) | bearer token required on `/sandboxes*`, `/drives*`, `/snapshots*` — **unset means the API is completely open, see section 9** |
| `SANDKILN_DRIVES_DIR` | `~/sandkiln-tools/drives` | where persistent drives are stored |
| `SANDKILN_IDLE_TIMEOUT_SECS` | unset (disabled) | auto-**destroy** a sandbox after this many idle seconds (no exec/read/write activity) — VM killed, network released, rootfs deleted, state gone for good; `0` also disables it |
| `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` | unset (disabled) | auto-**suspend** an idle sandbox instead: pause + snapshot it (same as `POST /sandboxes/:id/snapshot`) and free its VM/vcpu/memory, keeping it resumable; `0` also disables it. If both this and `SANDKILN_IDLE_TIMEOUT_SECS` are set, this must be strictly smaller — auto-suspend always gets first crack at an idle sandbox, and the destroy timeout becomes a backstop for a sandbox whose auto-suspend keeps failing (see `core/crates/daemon/src/config.rs`'s `auto_suspend_timeout` doc comment) |
| `SANDKILN_LOG_FORMAT` | `pretty` | set to `json` for one JSON object per log line, for log pipelines that parse fields |
| `SANDKILN_PREVIEW_TIMEOUT_SECS` | `30` | how long a dev-server preview proxy request waits for the guest to respond before a `504` |
| `SANDKILN_JAILER_ENABLED` | unset (disabled) | boot sandboxes via Firecracker's jailer instead of a direct spawn — see "Optional: jailer-based sandbox boot" above |
| `SANDKILN_JAILER_BIN` | `~/sandkiln-tools/bin/jailer` | path to the jailer binary (must be setuid-root, see above) |
| `SANDKILN_JAILER_CHROOT_BASE_DIR` | `~/sandkiln-tools/jail` | where per-VM chroots are created |
| `SANDKILN_JAILER_UID_GID_BASE` | `600000` | first uid/gid in the range dedicated to jailed VMs |
| `SANDKILN_JAILER_POOL_SIZE` | `32` | how many distinct uid/gid pairs (= max concurrent jailed sandboxes) |

Paths accept a leading `~/` and are expanded against `$HOME` at startup —
useful when scripting, but be aware this means `$HOME` must be set
correctly for whichever user actually runs the process (see the `sudo
-E` note in section 10 for where this has bitten before).

## 8. Starting the daemon

**One command**, once sections 3–7 are done:

```
SANDKILN_AUTH_TOKEN=$(openssl rand -hex 32) scripts/sandkilnd-ctl.sh start
```

`scripts/sandkilnd-ctl.sh` combines everything the steps above did by
hand into one repeatable command: builds `sandkilnd` (fast/no-op if
nothing changed), runs `preflight-check.sh` and refuses to start on a
`FAIL`, grants `CAP_NET_ADMIN` via `sudo` if the binary doesn't already
have it, starts the daemon in the background, and waits for `/healthz`
to actually respond before returning — so "the command exited 0" means
"the daemon is really up," not just "a process was spawned." It passes
through every `SANDKILN_*` env var untouched, the same as running
`sandkilnd` directly.

```
scripts/sandkilnd-ctl.sh status      # is it running, is it healthy
scripts/sandkilnd-ctl.sh logs [-f]   # tail its log
scripts/sandkilnd-ctl.sh restart     # stop, then start again — also
                                      # reaps any Firecracker process the
                                      # daemon left orphaned on the way down
scripts/sandkilnd-ctl.sh stop
```

Safe to run repeatedly: `start` on an already-running daemon just prints
its PID and exits; `stop` on a stopped one is a no-op. `start --no-build`
skips the build step (if you've already built it yourself);
`start --no-preflight` skips the check (for when you've already confirmed
everything and just want it back up fast).

Prefer to run it by hand instead — `SANDKILN_AUTH_TOKEN=... core/target/release/sandkilnd`
in a foreground shell — and that still works exactly as before; the
control script is a convenience, not a requirement. For a persistent,
supervised deployment that survives a reboot, skip straight to section 11
instead of either of the above.

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
npm install sandkiln
```
```
pip install sandkiln
```

The CLI installs as `sandkiln-cli` (the bare name `kiln` was already
taken by an unrelated package — see `packages/cli/AGENTS.md`) but the
command itself is still `kiln`:
```
npm install -g sandkiln-cli
kiln sandbox create --base-url http://127.0.0.1:7777 --token $SANDKILN_AUTH_TOKEN
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

- **`scripts/setup.sh` fails with "a terminal is required to read the
  password"** — it ran non-interactively (e.g. over `ssh host 'cmd'`
  rather than an interactive session) and one of its `sudo` steps had no
  TTY to prompt on. Run it from a real interactive shell, or
  `sudo -v` first to cache credentials in that session before invoking it
  non-interactively.
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
- **Sandbox creation fails with a jailer-related error after enabling
  `SANDKILN_JAILER_ENABLED`** — almost always the setuid bit on the
  `jailer` binary: check `ls -la ~/sandkiln-tools/bin/jailer` shows
  `-rwsr-xr-x` owned by `root`, and re-apply `chown root:root` /
  `chmod u+s` if you re-ran `install-firecracker.sh` since. If it's set
  correctly and jailer still fails, check `mount | grep cgroup2` — jailer
  is invoked with `--cgroup-version 2` unconditionally, and fails if the
  host is still on the legacy cgroups v1 hierarchy.
- **`400 snapshotting a jailed sandbox is not supported yet`** — expected;
  see "Optional: jailer-based sandbox boot" above. Stop the sandbox
  instead of snapshotting it if you don't need to resume it.

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
- **Firecracker's jailer** (chroot, cgroups v2 resource limits, a
  dedicated unprivileged uid/gid per VM) is available but opt-in and off
  by default — see "Optional: jailer-based sandbox boot" in section 6.
  With it off (the default), every sandbox boots via a direct Firecracker
  process spawn, with isolation coming from KVM/hardware virtualization
  itself plus the network controls above, not from jailer hardening on
  top. As of this writing jailer's actual chroot/cgroup/uid-drop behavior
  has been verified against real unit tests but not a real installed
  jailer binary on real hardware — see the recommendation at the end of
  section 6 before relying on it for a genuinely adversarial multi-tenant
  workload. Either way, snapshotting a jailed sandbox isn't supported yet
  (`400`).
- **No per-sandbox resource ceiling enforcement yet** — `vcpu_count`/
  `mem_size_mib` apply uniformly to every sandbox via daemon config;
  nothing today stops a caller from running a sandbox indefinitely except
  the optional `SANDKILN_IDLE_TIMEOUT_SECS` (destroy) and
  `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` (suspend).
- **A sandbox can disappear on its own, not just via an explicit `stop()`
  or `snapshot()`** — with `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` set, an
  idle sandbox is paused and snapshotted automatically, the same as a
  manual `snapshot()` call: it vanishes from `GET /sandboxes` and a new
  `Snapshot` takes its place. Look it up via `GET
  /snapshots?source_sandbox_id=<the sandbox id you had>` to find the
  resulting snapshot id and `resume()`/`fork()` it. If auto-suspend itself
  fails partway through (out of disk, a Firecracker error), the sandbox is
  stopped and its resources released as a fallback rather than left
  half-paused — it will not appear as a snapshot in that case, since
  there's nothing valid on disk to represent.
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

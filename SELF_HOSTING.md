# Self-hosting sandkiln

There's no hosted sandkiln service — every instance is self-hosted. This is
the full path from a bare Linux box to a working `sandkilnd` daemon you can
create sandboxes against. It matches exactly how the dev box for this
project itself is set up; nothing here is aspirational.

## What you need

- A Linux host with `/dev/kvm` present and read/write for the user running
  the daemon (`ls -la /dev/kvm`; add your user to the `kvm` group if not).
  Bare metal or a VM with nested virtualization enabled both work —
  Firecracker just needs KVM.
- `x86_64` or `aarch64`. Firecracker ships binaries for both.
- Root (or sudo) for one-time setup steps only — the daemon itself runs
  unprivileged afterward (see "Why not just run as root" below).
- Rust (stable, via `rustup`) to build `core/`, and Node.js 20+ if you also
  want to build the CLI/SDK from source instead of installing them from
  npm.

## 1. Build the workspace

```
cd core
cargo build --release --workspace
```

This produces `sandkilnd` (the daemon), `sandkiln-guest-agent` (baked into
the rootfs image, not run on the host), and the `vmm`/`protocol` libraries
they depend on.

## 2. Install Firecracker

```
scripts/install-firecracker.sh ~/sandkiln-tools
```

Downloads `firecracker` and `jailer` into `~/sandkiln-tools/bin`. Pin a
specific version with `FIRECRACKER_VERSION=v1.16.1` if you need one other
than the script's default.

## 3. Build the base rootfs image

```
sudo images/build-universal-image.sh
```

Builds a production rootfs from scratch with `debootstrap`: current Ubuntu
LTS, current Node.js LTS, Python 3, common CLI tooling, a working systemd
init, and the guest agent baked in as a systemd service. Needs sudo and
meaningful disk space (~6GB free during the build). This is what
`SANDKILN_BASE_ROOTFS` should point at — do not use
`images/fetch-test-image.sh`'s CI test image for anything beyond manual
boot testing; it's missing even `ca-certificates`.

`build-universal-image.sh` also runs `setup-multi-agent-users.sh` as its
last step, baking in `agent0`..`agentN` isolated user accounts. Run that
script standalone against an already-built image if you need to add it
later.

You'll also need a kernel — `images/fetch-test-image.sh` pulls a known-good
one from Firecracker's public CI artifacts if you don't already have one.

## 4. Set up sandbox networking

The daemon runs unprivileged and only ever leases/attaches *existing* tap
devices — creating new ones needs a real root-owned `TUNSETIFF` ioctl that
ambient capabilities don't cover. So tap devices are created once, as root:

```
sudo scripts/create-tap-pool.sh 32 <your-username> sktap
```

This creates `sktap0`..`sktap31` owned by your user. `32` is your
concurrent-sandbox-with-networking ceiling — raise it and
`SANDKILN_TAP_POOL_SIZE` together if you need more. The daemon itself
creates and manages the bridge (`sktapbr0` by default) and NAT rules at
startup via `NetworkManager::ensure_ready()` — you don't need to run
`scripts/setup-tap-network.sh` separately; that script is for the older
point-to-point (one tap, no bridge) model used by
`scripts/boot-test-vm.sh` during manual testing, not the daemon's bridged
pool model.

Guest DNS needs a forwarder, since guests often can't reach public
resolvers directly but can reach the host's own resolver:

```
sudo scripts/start-dns-proxy.sh 172.16.0.1   # match SANDKILN_BRIDGE_GATEWAY
```

Run this once per boot (it's not a systemd unit yet — see `ROADMAP.md`'s
observability/ops section).

## 5. Grant the daemon CAP_NET_ADMIN

```
sudo scripts/grant-net-admin.sh core/target/release/sandkilnd
```

This is what lets the daemon manage tap attachment, the bridge, and
iptables rules without running as root. **It does not survive a rebuild**
— re-run this after every `cargo build --release` that produces a new
`sandkilnd` binary, or every network operation will fail with "Operation
not permitted."

### Why not just run as root

Running the whole daemon as root would sidestep all of this, but it means
every bug in an HTTP-facing Rust binary is a root-level bug. Ambient
`CAP_NET_ADMIN` gets the daemon exactly the one privilege it actually
needs (network device management) and nothing else — the blast radius of
a compromised daemon process is much smaller. This is also why tap device
*creation* is a separate one-time root step rather than something the
daemon does itself: it's the one operation ambient `CAP_NET_ADMIN` alone
doesn't cover, and doing it once ahead of time avoids needing to run the
daemon with anything beyond that one capability.

## Optional: enable jailer-based sandbox boot

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

## 6. Configure and run the daemon

All configuration is env vars, all optional with sane defaults (see
`core/crates/daemon/src/config.rs`):

| Variable | Default | What it controls |
|---|---|---|
| `SANDKILN_LISTEN_ADDR` | `127.0.0.1:7777` | HTTP listen address |
| `SANDKILN_FIRECRACKER_BIN` | `~/sandkiln-tools/bin/firecracker` | path to the Firecracker binary |
| `SANDKILN_KERNEL_PATH` | `~/sandkiln-tools/images/vmlinux-5.10.223` | guest kernel image |
| `SANDKILN_BASE_ROOTFS` | `~/sandkiln-tools/images/ubuntu-22.04.ext4` | base rootfs cloned per sandbox |
| `SANDKILN_VCPU_COUNT` | `2` | vCPUs per sandbox |
| `SANDKILN_MEM_SIZE_MIB` | `512` | memory per sandbox |
| `SANDKILN_BRIDGE_NAME` | `sktapbr0` | shared bridge the daemon creates |
| `SANDKILN_BRIDGE_GATEWAY` | `172.16.0.1` | bridge/gateway IP (`/24` subnet) |
| `SANDKILN_UPLINK_IFACE` | auto-detected | host interface sandboxes NAT out through |
| `SANDKILN_TAP_POOL_PREFIX` | `sktap` | must match `create-tap-pool.sh`'s prefix |
| `SANDKILN_TAP_POOL_SIZE` | `32` | must match `create-tap-pool.sh`'s count |
| `SANDKILN_AUTH_TOKEN` | unset (auth disabled) | bearer token required on `/sandboxes*`, `/drives*` |
| `SANDKILN_DRIVES_DIR` | `~/sandkiln-tools/drives` | where persistent drives are stored |
| `SANDKILN_IDLE_TIMEOUT_SECS` | unset (disabled) | stop a sandbox automatically after this many seconds with no exec/read-file/write-file activity; `0` also disables it |
| `SANDKILN_JAILER_ENABLED` | unset (disabled) | boot sandboxes via Firecracker's jailer instead of a direct spawn — see "Optional: enable jailer-based sandbox boot" above |
| `SANDKILN_JAILER_BIN` | `~/sandkiln-tools/bin/jailer` | path to the jailer binary (must be setuid-root, see above) |
| `SANDKILN_JAILER_CHROOT_BASE_DIR` | `~/sandkiln-tools/jail` | where per-VM chroots are created |
| `SANDKILN_JAILER_UID_GID_BASE` | `600000` | first uid/gid in the range dedicated to jailed VMs |
| `SANDKILN_JAILER_POOL_SIZE` | `32` | how many distinct uid/gid pairs (= max concurrent jailed sandboxes) |

**Set `SANDKILN_AUTH_TOKEN` for anything reachable beyond localhost** — with
it unset the API is completely open. Auth is a no-op middleware when
unset, real bearer-token checking when set (`core/crates/daemon/src/auth.rs`).

```
SANDKILN_AUTH_TOKEN=$(openssl rand -hex 32) \
  core/target/release/sandkilnd
```

## 7. Verify it works

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

Or point the SDK/CLI at it instead of writing raw `curl`:

```
npm install -g kiln
kiln sandbox create
```

```python
pip install sandkiln
```

Set `SANDKILN_API_URL`/equivalent per-client config to your daemon's
address — see `packages/sdk`, `packages/python`, and `packages/cli` for
each client's exact configuration surface.

## Running the daemon as a persistent service

Not shipped as a systemd unit yet (tracked in `ROADMAP.md`'s
"Observability" / ops section) — for now, a minimal unit file works:

```ini
[Unit]
Description=sandkiln daemon
After=network.target

[Service]
ExecStart=/path/to/sandkilnd
Environment=SANDKILN_AUTH_TOKEN=<your-token>
Restart=on-failure
User=<your-username>

[Install]
WantedBy=multi-user.target
```

Remember: `AmbientCapabilities=CAP_NET_ADMIN` in the unit is *not* a
substitute for `grant-net-admin.sh` — the daemon raises the capability
itself at startup from what `setcap` granted the binary file; a systemd
unit granting it independently hasn't been tested against this code path
and may not interact correctly with the daemon's own
`caps::raise(...Ambient...)` call. Run `grant-net-admin.sh` regardless.

## Troubleshooting

- **"Operation not permitted" on any network call** — you rebuilt the
  daemon and forgot to re-run `grant-net-admin.sh`. This is the single
  most common self-hosting mistake; see the gotcha in `AGENTS.md`.
- **`tap devices missing: [...] — run scripts/create-tap-pool.sh first`**
  at startup — `SANDKILN_TAP_POOL_PREFIX`/`SANDKILN_TAP_POOL_SIZE` don't
  match what you actually created, or the pool was created for a
  different user.
- **Sandboxes boot but have no network** — check `SANDKILN_UPLINK_IFACE`
  detected the right interface (`ip route show default`), and that
  `start-dns-proxy.sh` is running for guest DNS to work.
- **`/dev/kvm: permission denied`** — add your user to the `kvm` group
  (`sudo usermod -aG kvm $USER`, then re-login).
- **Sandbox creation fails with a jailer-related error after enabling
  `SANDKILN_JAILER_ENABLED`** — almost always the setuid bit on the
  `jailer` binary: check `ls -la ~/sandkiln-tools/bin/jailer` shows
  `-rwsr-xr-x` owned by `root`, and re-apply `chown root:root` /
  `chmod u+s` if you re-ran `install-firecracker.sh` since. If it's set
  correctly and jailer still fails, check `mount | grep cgroup2` — jailer
  is invoked with `--cgroup-version 2` unconditionally, and fails if the
  host is still on the legacy cgroups v1 hierarchy.
- **`400 snapshotting a jailed sandbox is not supported yet`** — expected;
  see "Optional: enable jailer-based sandbox boot" above. Stop the
  sandbox instead of snapshotting it if you don't need to resume it.

For anything not covered here, `AGENTS.md` documents every non-obvious
gotcha this project hit and fixed during development, with the reasoning
behind each fix.

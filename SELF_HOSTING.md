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

For anything not covered here, `AGENTS.md` documents every non-obvious
gotcha this project hit and fixed during development, with the reasoning
behind each fix.

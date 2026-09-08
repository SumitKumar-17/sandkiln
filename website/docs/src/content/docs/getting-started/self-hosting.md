---
title: Self-hosting quickstart
description: Get a sandkilnd daemon running from a bare Linux host — step by step, with the failures you're likely to actually hit.
---

There's no hosted sandkiln service — every instance is self-hosted. This page is a complete, step-by-step path from a bare Linux host to a real daemon booting real microVMs, including the specific failures people actually hit and how to fix them. For the full section-by-section reference (every environment variable, the networking internals, running as a persistent systemd service), see [`SELF_HOSTING.md`](https://github.com/SumitKumar-17/sandkiln/blob/main/SELF_HOSTING.md) in the repository — this page is the condensed, hands-on version of it.

## 1. Check your host actually supports this

- **Linux, x86_64.** Nothing here is built or verified for aarch64.
- **`/dev/kvm`, readable and writable by your user.** Bare metal or a VM with nested virtualization enabled both work — check with:
  ```bash
  ls -la /dev/kvm
  ```
  If it's not there at all, your host (or your cloud VM's instance type) doesn't have KVM/nested virtualization enabled — this is a hard requirement, not something to work around. If it's there but you can't read/write it:
  ```bash
  sudo usermod -aG kvm $USER
  # then log out and back in -- group membership doesn't apply to an already-open session
  ```
- **`sudo`, for one-time host setup only.** The daemon itself never runs as root once it's running — see [Privilege model](../../architecture/privilege-model/) for exactly why.
- **Rust** (via [rustup](https://rustup.rs)), plus the musl target for the guest agent:
  ```bash
  rustup target add x86_64-unknown-linux-musl
  sudo apt install musl-tools   # Debian/Ubuntu; adjust for your distro
  ```

Run `scripts/preflight-check.sh` at any point — it reads the same `SANDKILN_*` environment variables the daemon itself uses and tells you exactly what's missing, before you try to start anything.

## 2. Clone and build

```bash
git clone https://github.com/SumitKumar-17/sandkiln.git
cd sandkiln/core
cargo build --release --workspace
```

This builds `sandkilnd` (at `core/target/release/sandkilnd`) and the libraries it depends on. It does **not** build the guest agent yet — that happens automatically in the next step, cross-compiled for the guest's musl target.

## 3. One command: build the kernel/rootfs, wire up networking, start the daemon

```bash
cd ..   # back to the repo root
scripts/setup.sh
scripts/sandkilnd-ctl.sh start
```

`setup.sh` is idempotent (safe to re-run) and does everything that used to be a dozen manual steps with paths that had to match by hand: fetches a known-good kernel and a small test rootfs, injects the guest agent into it, creates a persistent tap device pool for sandbox networking, and grants the daemon `CAP_NET_ADMIN` — the one Linux capability it needs, not root.

That's a real, working daemon end to end — but booting from the small Firecracker CI test image (~300MiB, missing `ca-certificates` and any language runtime, fine for proving the stack works, not for real workloads). For a production image with Ubuntu, current Node.js/Python, and common tooling (needs `sudo`, ~8GiB free disk, and several minutes):

```bash
scripts/setup.sh --production
```

## 4. Verify it worked

```bash
curl http://127.0.0.1:7777/healthz
# ok

curl -X POST http://127.0.0.1:7777/sandboxes -d '{}'
# {"id": "..."}
```

Got an id back? The daemon is real and working. Move on to your language of choice: [JS/TS](../js/), [Python](../python/), or the [CLI](../cli/).

## 5. Before exposing this beyond localhost

Set an auth token — with none set, **every route except `/healthz` and `/metrics` is completely open** to anyone who can reach the port (create sandboxes, read/write files inside them, delete persistent drives). The daemon logs a startup warning specifically so this is never silent.

```bash
SANDKILN_AUTH_TOKEN=$(openssl rand -hex 32) scripts/sandkilnd-ctl.sh restart
```

Every client (both SDKs, the CLI, raw HTTP) sends this back as `Authorization: Bearer <token>` — see [Auth](../../concepts/auth/). This is a single shared secret, not a multi-tenant identity system — fine for a self-hosted, single-operator daemon, not a substitute for per-caller scoping (not implemented yet).

## When something goes wrong

- **`setup.sh` fails with "a terminal is required to read the password"** — it ran non-interactively (e.g. over `ssh host 'cmd'`) and a `sudo` step had no TTY to prompt on. Run it from a real interactive shell, or `sudo -v` first to cache credentials before running it non-interactively.
- **`/dev/kvm: permission denied`** — `sudo usermod -aG kvm $USER`, then log in again.
- **"Operation not permitted" on any network call** — you rebuilt the daemon and are running it manually (not via `sandkilnd-ctl.sh`, which handles this) and need to re-grant `CAP_NET_ADMIN`: `sudo scripts/host-setup/grant-net-admin.sh core/target/release/sandkilnd`.
- **`tap devices missing: [...]`** at startup — `SANDKILN_TAP_POOL_PREFIX`/`SANDKILN_TAP_POOL_SIZE` don't match what was actually created, or the pool was created for a different user than the daemon runs as.
- **Sandboxes create fine but `exec`/read/write time out or fail with a vsock error** — the guest agent isn't baked into whatever rootfs is configured. Run `sudo -E scripts/preflight-check.sh --root-checks` to confirm.
- **Sandboxes boot but have no outbound network** — check `ip route show default` picked the right interface, and confirm `scripts/host-setup/start-dns-proxy.sh` is actually running (DNS and raw IP connectivity are independent failure modes — check both separately).
- **`preflight-check.sh --root-checks` reports missing binaries you know are installed** — you ran it with plain `sudo`, which resets `$HOME` to `/root` and breaks every `~/sandkiln-tools/...` default path. Use `sudo -E` instead.

The repository's [`SELF_HOSTING.md`](https://github.com/SumitKumar-17/sandkiln/blob/main/SELF_HOSTING.md) covers more (production image build failures, jailer setup, running as a systemd service, the full environment-variable reference) if you hit something not listed here.

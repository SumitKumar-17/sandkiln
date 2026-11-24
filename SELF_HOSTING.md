# Self-hosting sandkiln

There is no hosted service. You build the daemon, prepare a kernel and
rootfs, set up host networking, and run `sandkilnd` yourself. This page is
the short path; the detailed reference lives in the docs site (links at the
bottom).

## Requirements

- Linux x86_64 with `/dev/kvm` readable and writable by the daemon's user
  (`sudo usermod -aG kvm $USER`, then log in again). Bare metal and nested
  virtualization both work.
- `sudo` for one-time host setup. The daemon itself never runs as root; it
  holds only `CAP_NET_ADMIN`.
- Rust (stable, via `rustup`). `scripts/setup.sh` adds the musl target and
  installs `musl-tools`.
- About 8GiB of free disk for `--production`; under 500MiB otherwise.

`scripts/preflight-check.sh` reports what is missing, using the same
`SANDKILN_*` variables and defaults as the daemon.

## Quick start

```bash
git clone https://github.com/SumitKumar-17/sandkiln.git
cd sandkiln
scripts/setup.sh                 # idempotent: build, Firecracker, kernel, test image,
                                 # agent injection, tap pool, DNS forwarder, capabilities
scripts/sandkilnd-ctl.sh start   # returns once /healthz answers
```

The default image is a small test image (about 300MiB, no language runtime
or `ca-certificates`). For a real Ubuntu image with Node.js and Python, run
`scripts/setup.sh --production` (needs sudo, several minutes).

Daemon control: `scripts/sandkilnd-ctl.sh status | logs [-f] | restart | stop`.

## Configuration

Settings resolve as `SANDKILN_<KEY>` environment variable, then
`sandkiln.toml`, then the built-in default. `setup.sh` writes a starter
`sandkiln.toml` at the repo root; [`sandkiln.example.toml`](sandkiln.example.toml)
lists every key with its default.

```bash
sandkilnd --check-config      # validate (exit 0 ok, 2 invalid)
sandkilnd --print-config      # effective value and source of every setting
sandkilnd --config-reference  # every setting with its description
```

Set `auth_token` before exposing the daemon beyond localhost. With none, every
route except `/healthz` and `/metrics` is open. Keep the file `chmod 600`.

## Verify

```bash
curl http://127.0.0.1:7777/healthz
curl -s -X POST http://127.0.0.1:7777/sandboxes -d '{}'      # {"id":"..."}
curl -s -X POST http://127.0.0.1:7777/sandboxes/<id>/exec \
  -H 'Content-Type: application/json' \
  -d '{"command":"echo","args":["hello"]}'
```

For full API coverage run `scripts/integration-test.sh`. Clients: `npm install
sandkiln`, `pip install sandkiln`, or `npm install -g sandkiln-cli` (the
command is `kiln`).

## Running as a service

```bash
sudo scripts/install-systemd-service.sh \
  $(pwd)/core/target/release/sandkilnd <run-as-user> /etc/sandkiln/sandkilnd.env
sudo systemctl enable --now sandkilnd
```

The unit runs unprivileged, grants `CAP_NET_ADMIN` via `AmbientCapabilities=`
(nothing to redo after a rebuild), runs preflight before each start, and
restarts on failure.

## Upgrading

- Daemon source: `cargo build --release --workspace`. For direct runs also
  re-run `sudo scripts/host-setup/grant-net-admin.sh core/target/release/sandkilnd`.
- Guest agent: rebuild and re-inject into every rootfs in use
  (`scripts/dev.sh inject-agent [rootfs]`).
- Then re-run `scripts/preflight-check.sh`.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `/dev/kvm: permission denied` | Join the `kvm` group and log in again. |
| "Operation not permitted" on network calls | Direct run after a rebuild: re-run `grant-net-admin.sh`, or use systemd. |
| `tap devices missing` at startup | `tap_pool_*` must match `create-tap-pool.sh`. |
| `exec` or file calls time out | Guest agent missing from the rootfs: `sudo -E scripts/preflight-check.sh --root-checks`. |
| No outbound network | Check `ip route show default` and `systemctl status sandkiln-dnsmasq`. |

More in [Operations and security](docs-site/self-hosting/operations.mdx).

## Detailed reference

- [Self-hosting quickstart](docs-site/getting-started/self-hosting.mdx)
- [Manual host setup](docs-site/self-hosting/manual-setup.mdx): images, kernel, mounts, networking, capabilities, jailer
- [Configuration](docs-site/self-hosting/configuration.mdx)
- [Operations and security](docs-site/self-hosting/operations.mdx): upgrades, troubleshooting, persistent state, security posture

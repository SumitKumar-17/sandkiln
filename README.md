# sandkiln

Run untrusted or AI-generated code in hardware-isolated Firecracker microVMs, through an HTTP API, SDKs and a CLI.

Each sandbox is a real microVM with its own kernel, filesystem and network interface. You run the `sandkilnd` daemon yourself on a Linux host with KVM; there is no hosted service. Clients create a sandbox, run commands, read and write files, snapshot it, and stop it.

- Docs: https://sandkiln.mintlify.site/
- Website and engineering deep-dives: https://sandkiln.vercel.app

## Quickstart

Needs a Linux x86_64 host with `/dev/kvm` readable and writable by your user, `sudo` for one-time setup, Rust via [rustup](https://rustup.rs), and `dnsmasq-base` (guest DNS forwarder). `setup.sh` installs `musl-tools` with `apt` if it is missing.

```bash
git clone https://github.com/SumitKumar-17/sandkiln.git
cd sandkiln
scripts/setup.sh                 # build, Firecracker, test kernel+rootfs, tap pool, DNS forwarder, capabilities, sandkiln.toml; idempotent
scripts/sandkilnd-ctl.sh start   # starts the daemon and waits for /healthz

curl -s -X POST http://127.0.0.1:7777/sandboxes -d '{}'
# {"id":"<sandbox-id>"}
curl -s -X POST http://127.0.0.1:7777/sandboxes/<sandbox-id>/exec \
  -H 'Content-Type: application/json' \
  -d '{"command":"echo","args":["hello from a microVM"]}'
```

`setup.sh` boots from a small test image (about 300MiB, no language runtimes). For a real Ubuntu image with Node.js and Python, run `scripts/setup.sh --production` (needs about 8GiB of disk and several minutes). Full details and every setting: [SELF_HOSTING.md](SELF_HOSTING.md).

Configuration is `sandkiln.toml` (written by `setup.sh`; every key in [sandkiln.example.toml](sandkiln.example.toml)), overridden by `SANDKILN_<KEY>` environment variables. `sandkilnd --check-config` validates it and `--print-config` shows the effective values.

With no `auth_token` set, every route except `/healthz` and `/metrics` is open to anyone who can reach the port. Set one before listening beyond localhost.

From TypeScript (`npm install sandkiln`):

```ts
import { Sandbox } from "sandkiln";

const sandbox = await Sandbox.create({ tags: { env: "demo" } });
const result = await sandbox.runCommand("python3", ["-c", "print(21 * 2)"]);
console.log(result.stdout.trim(), result.exitCode); // 42 0
await sandbox.stop(); // kept as a resumable snapshot by default
```

Python (`pip install sandkiln`) mirrors the same API with snake_case names. The CLI (`npm install -g sandkiln-cli`, command `kiln`) covers the same operations: `kiln sandbox create`, `kiln sandbox exec <id> -- <cmd>`. The `python3` example needs the production image.

## What works

Status as of 2026-11-23, verified against a real daemon and real microVMs on one dev box. `ROADMAP.md` is the authoritative list.

| Area | Status |
| --- | --- |
| Create, exec, file operations, list, stop | Working in the HTTP API, both SDKs and the CLI |
| Snapshot, resume, fork, named sandboxes, durable across daemon restart | Working |
| Pre-warmed snapshot pools (`max_count`, queueing) | Working; pool config is in-memory, lost on daemon restart |
| Per-sandbox networking: tap pool, shared bridge with port isolation, NAT, egress policy via iptables chains | Working |
| Persistent drives with read-only sharing, custom and managed images | Working |
| Idle lifecycle: auto-suspend, auto-destroy, archive | Working; daemon-operator config only, no client surface |
| Interactive PTY over WebSocket, streamed exec | Working in the JS SDK and CLI; not in the Python SDK |
| Dev-server preview proxy, local tunnel, remote storage mounts (S3-compatible via FUSE and rclone) | Working |
| Snapshot lineage, time-travel restore | HTTP API only; no SDK or CLI surface yet |
| Firecracker jailer hardening | Opt-in; not yet proven on real hardware |
| Request ids, `/metrics`, durable lifecycle history (sqlite) | Working |
| Orderly shutdown: on `SIGTERM` the daemon drains requests, then snapshots every live sandbox so it is resumable after restart | Working; a `SIGKILL` or crash still orphans Firecracker processes |
| OCI image conversion, multi-node, snapshot encryption at rest | Planned |
| Per-caller auth scoping | Not implemented; one shared bearer token, compared in constant time |

Tests: `cargo test --workspace` (340 passing, no KVM needed) and `scripts/integration-test.sh` (about 320 checks against a live daemon; `SANDKILN_INTEGRATION_TEST_ONLY=<substring>` runs one topic).

## Measured performance

One shared dev box (Intel i9-13900K, 62 GiB, ext4 host, Firecracker v1.16.1), small samples, directional only. The latency a caller feels is time to first command; the `POST /sandboxes` response itself returns before the guest is ready.

| Measurement | Result | Source |
| --- | --- | --- |
| **Time to first command**: `POST /sandboxes` to the first successful `exec`, cold sandbox | 413ms before, 214.5ms after trimming guest boot (2026-11-24, `scripts/bench-report.sh 20`); about 125ms of what remains is guest systemd userspace | `first_exec_client` |
| Time to first command, sandbox resumed from a snapshot | 18ms before, 8.4ms after | `first_exec_resumed` |
| Cold `POST /sandboxes`, rootfs is a full byte copy on ext4 | 187.5ms mean of 20 (clone 127.9ms, boot 14.9ms, network lease 3.5ms) | `scripts/bench-report.sh` |
| Cold `POST /sandboxes`, rootfs dir on the same XFS reflink filesystem as the base image | 17.9ms mean of 20 on 2026-11-23 (clone 0.08ms); re-run 18.4ms (clone 0.084ms, lease 3.1ms, boot 13.4ms) | same |
| Later `exec` on the same sandbox | about 3-5ms | same |
| Snapshot take, `criterion` | 485-727ms (re-measured 2026-11-23; an earlier run read 310-335ms, and under load 550-915ms) | `cargo bench -p sandkiln-vmm --bench vm_lifecycle` |
| Snapshot via `POST /sandboxes/:id/snapshot` and stop | 6.8s to 0.56s end to end after the guest `sync` was dropped for a paused VM | `scripts/integration-tests/25-long-running-commands.sh` |
| Firecracker spawn to `InstanceStart`, `criterion` (an internal phase, not the time a caller waits) | 12.6-14.0ms (2026-11-23; an earlier run read 10.5-10.9ms) | same bench |
| Resume from snapshot, `criterion` | 3.7-4.0ms | same bench |
| `exec` round trip on an open connection, `criterion` | 242-248us | same bench |

To get the 17.9ms path, put `SANDKILN_ROOTFS_DIR` on the same reflink-capable filesystem as `SANDKILN_BASE_ROOTFS`. On an ext4-only host, `sudo scripts/host-setup/create-reflink-volume.sh <user>` builds an XFS loopback volume. `/metrics` reports `rootfs_clone_total{method="reflink"|"copy"}` so a silent fallback is visible. See [Startup latency](https://sandkiln.mintlify.site/architecture/startup-latency).

## Layout

```
core/crates/protocol/    wire format shared by host and guest
core/crates/guest-agent/ static musl binary that runs inside the VM
core/crates/vmm/         drives Firecracker, rootfs clones, networking (host side)
core/crates/store/       durable sandbox-lifecycle history (sqlite)
core/crates/daemon/      axum HTTP API (sandkilnd)
packages/sdk/            sandkiln on npm (TypeScript)
packages/python/         sandkiln on PyPI
packages/cli/            kiln CLI, wraps the JS SDK
images/                  kernel and rootfs build scripts, agent injection
scripts/                 setup, daemon control, tests, benchmarks
website/                 project site (Astro)
docs-site/               full docs (Mintlify)
examples/                runnable reference projects
```

## Links

- [SELF_HOSTING.md](SELF_HOSTING.md): host setup, every `SANDKILN_*` setting, running under systemd
- [ROADMAP.md](ROADMAP.md): current state, measurements and open work
- [CHANGELOG.md](CHANGELOG.md)
- [AGENTS.md](AGENTS.md): engineering standard for contributors, human or agent

## License

MIT

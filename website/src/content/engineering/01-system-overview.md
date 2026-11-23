---
title: "System overview: crates, ownership and the request path"
summary: "How the five Rust crates divide responsibility between host and guest, which direction dependencies may point, and what happens to one request from the SDK down to a Firecracker microVM and back."
order: 1
tags: ["architecture", "crates", "ownership", "lifecycle"]
updated: 2026-11-24
---

sandkiln runs untrusted or AI-generated code inside Firecracker microVMs. A caller talks HTTP to a daemon; the daemon boots a VM; a small agent inside the VM runs the commands. Almost every design decision in the codebase follows from three questions: what must run inside the guest, what must run on the host with privileges, and which crate is allowed to know about which. This post is the map. The following posts go deep on the [vsock protocol](/engineering/02-vsock-protocol-and-guest-agent) and on [driving Firecracker](/engineering/03-driving-firecracker).

## The five crates

The Rust workspace lives under `core/` and has exactly five members (`core/Cargo.toml`):

| Crate | Runs where | Owns |
| --- | --- | --- |
| `sandkiln-protocol` | both | Wire messages, length-prefixed framing, the four vsock port numbers |
| `sandkiln-guest-agent` | guest | Executing commands, file operations, PTY, streamed exec, tunnel listener |
| `sandkiln-vmm` | host | Spawning and configuring Firecracker, vsock client, tap leases, rootfs clones, jailer, drives, images, egress rules |
| `sandkiln-store` | host | Durable sandbox lifecycle history in sqlite |
| `sandkiln-daemon` | host | The HTTP API (`sandkilnd`), configuration, live-sandbox bookkeeping, pools, snapshots, background reapers, orderly shutdown |

The binary that matters most for scale is `sandkiln-guest-agent`. It is built for a static musl target and optimised for size (`opt-level = "z"` in the workspace profile, `core/Cargo.toml:24`) because it ships inside every rootfs image. Everything else is a normal host binary or library.

## Dependency direction

```mermaid
flowchart LR
    protocol["sandkiln-protocol"]
    guest["sandkiln-guest-agent"]
    vmm["sandkiln-vmm"]
    store["sandkiln-store"]
    daemon["sandkiln-daemon"]

    guest -->|"Request, Response, ports"| protocol
    vmm -->|"encode and decode helpers"| protocol
    daemon -->|"Request, Response"| protocol
    daemon -->|"Vm, NetworkManager, DriveStore"| vmm
    daemon -->|"HistoryStore"| store
```

Every arrow in that diagram is a real `path = "../..."` entry in a crate's `Cargo.toml`. The rules they encode:

1. **`protocol` depends on nothing but serde.** Its manifest lists `serde` and `serde_json` only. It has no knowledge of vsock, HTTP or Firecracker. A need that involves networking or processes belongs in `vmm` or `guest-agent`, never here. This keeps the one piece of code both sides compile from small enough to review as a contract.
2. **`guest-agent` and `vmm` never depend on each other.** They only meet through `protocol`. That is what lets the guest binary stay tiny and statically linked: it cannot accidentally pull in host-side crates, and the host cannot reach into guest internals. The agreement is purely the wire format.
3. **`vmm` has no async runtime.** Its dependencies are `sandkiln-protocol`, `libc`, `serde`, `serde_json` and `tracing`. Every call into it is blocking. The daemon bridges that boundary once, with `spawn_blocking`, rather than each vmm module inventing its own concurrency model.
4. **`store` is independent of `vmm`.** It records "this sandbox existed and ended this way" and knows nothing about VMs. It is a sibling the daemon composes, not a layer beneath the VM code.
5. **`daemon` is the only crate that depends on everything.** It parses requests, calls `vmm`, shapes responses. Lifecycle logic that is not HTTP-shaped (boot, stop, network leases, rootfs cloning) belongs in `vmm`.

A consequence worth naming: `vmm` does not track which sandbox holds which network lease. `NetworkManager` hands out a `Lease` and takes it back, but the mapping from sandbox to lease lives in the daemon's `Sandbox` records. Ownership of "who holds what" sits in one place, the daemon's `AppState`, which is also where the drive and image holder checks live.

## What lives on the host and what lives in the guest

The split is a security boundary, not a convenience.

The **guest** contains one process of ours, the agent, running as an ordinary systemd service inside the VM. It is a deliberately dumb executor: it runs what it is told, with no path validation, because a path is only meaningful inside that one microVM's own filesystem. It listens on four vsock ports and, for the tunnel feature, dials out on one.

The **host** contains everything with authority: the Firecracker processes, the tap devices and bridge, iptables rules, the rootfs files, the history database, the API surface, and the bearer-token check. The daemon runs unprivileged with only ambient `CAP_NET_ADMIN`, raised in a plain `fn main()` (`core/crates/daemon/src/main.rs`) before the Tokio runtime starts so worker threads inherit it. The bearer token is compared in constant time (`constant_time_eq` in `core/crates/daemon/src/http/auth.rs`), so response timing does not reveal how much of a guessed token was right. Tap devices are pre-created by a host-setup script and only leased by the daemon, because creating one needs real root and ambient `CAP_NET_ADMIN` only covers netlink operations on existing interfaces.

The control channel between them is vsock, not the guest's network interface. Firecracker exposes one virtio-vsock device per VM backed by a host Unix socket, so the control path is independent of the sandbox's IP configuration, and an egress policy that blocks all outbound traffic has no effect on it. The [vsock wire protocol page](https://sandkiln.mintlify.site/internals/vsock-wire-protocol) in the docs covers that design in prose.

## The request lifecycle at a glance

Creating a sandbox and then running a command in it exercises most of the stack. The sequence below shows the cold path (no pre-warmed pool involved).

```mermaid
sequenceDiagram
    participant SDK as SDK or kiln
    participant D as daemon create_sandbox
    participant R as vmm rootfs
    participant N as NetworkManager
    participant V as vmm Vm
    participant FC as Firecracker
    participant A as guest agent
    participant H as HistoryStore

    SDK->>D: POST /sandboxes
    par concurrent setup
        D->>R: clone_file(base image, per-sandbox rootfs)
        D->>N: lease() tap device and guest IP
    end
    D->>V: Vm::boot(VmConfig)
    V->>FC: spawn, then PUT /boot-source, /drives, /vsock
    V->>FC: PUT /actions InstanceStart
    V-->>D: Vm handle
    D->>H: record_created(id, name, tags, image)
    D-->>SDK: 201 with sandbox id
    SDK->>D: POST /sandboxes/id/exec
    D->>V: call_agent copies out AgentEndpoint, releases map lock
    D->>V: AgentEndpoint::call_with_timeout(Request::Exec)
    V->>A: CONNECT 5000, then framed Request
    A-->>V: framed Response::Exec
    V-->>D: stdout, stderr, exit_code
    D-->>SDK: JSON result
```

Walking it step by step:

1. The SDK sends `POST /sandboxes`. The daemon resolves the name and image, and for a request without drives or a rate limit it first tries to claim a warm slot from a pre-warmed pool (see the [pools page](https://sandkiln.mintlify.site/internals/pre-warmed-pools)). The diagram shows the cold branch.
2. The two independent setup steps, cloning the rootfs and leasing a tap plus IP, run concurrently on two scoped threads and are joined before boot. Each is individually timed and recorded in the `create_phase_duration_ms` metric family.
3. If the jailer is enabled, a uid/gid is leased from a `JailerIdPool` as the third resource. It is the last one acquired because nothing overlaps it.
4. `Vm::boot` spawns Firecracker, configures it over its API socket and starts it. On any failure it kills the child and removes the sockets and chroot itself before returning the error.
5. The daemon registers the sandbox in its in-memory map and writes a history row. A failed boot never reaches history.
6. Each later command goes through the daemon's shared `call_agent` helper (`core/crates/daemon/src/http/routes/exec.rs:122`). It finds the sandbox, touches its last-activity timestamp, copies out an `AgentEndpoint` and releases the sandbox map's lock, and only then enters `spawn_blocking` to call `AgentEndpoint::call_with_timeout`. That opens a fresh vsock connection, sends one framed request and waits up to `exec_timeout_secs` (default 600) for one framed response; expiry is an HTTP 504, not a 500. The vmm's own control-plane calls use a separate 3 second reply bound. See the [vsock protocol post](/engineering/02-vsock-protocol-and-guest-agent) for why those two bounds differ and why a request already sent to the guest is never retried.

## Ownership: who creates, who cleans up

Every resource has exactly one owner and one cleanup path.

| Resource | Created by | Released by | If the owner crashes |
| --- | --- | --- | --- |
| Per-sandbox rootfs file | `clone_file` in `vmm::rootfs`, called from the daemon's `clone_rootfs` | `storage::OwnedFile` deletes it on any failed create; the destroy path removes it for a live sandbox | file stays on disk under the rootfs dir |
| Tap device and guest IP | `NetworkManager::lease` | `NetworkManager::release`, called on boot failure and on stop | pool is rebuilt at next start; snapshots re-reserve theirs via `reserve` |
| Firecracker process, sockets, jail dir | `Vm::boot` | `Vm::stop` or `Vm::force_stop`, or boot's own failure cleanup | on SIGKILL or a crash the process is orphaned; no re-adoption exists |
| Jailer uid/gid | `JailerIdPool::lease` | `JailerIdPool::release` in the destroy path | in-memory pool resets |
| Egress chain (iptables) | `vmm::egress::apply` | `vmm::egress::remove` before lease release | rules remain until removed |
| History row | `HistoryStore::record_created` | `record_ended`, or marked at next startup | row marked `OrphanedByRestart` |

The "crash" column is honest about a hard limit. A live `Sandbox` owns an OS process with no serialisable form, so a hard death of the daemon (`SIGKILL`, a panic, a power loss) orphans every live sandbox's Firecracker process, and nothing re-adopts them. A planned stop is different. On `SIGTERM` or `SIGINT` the daemon stops accepting connections, gives in-flight requests 30 seconds, and then snapshots every live sandbox concurrently, destroying only those that structurally cannot be snapshotted (jailed ones, and forks that do not own their rootfs or lease), all under a 60 second deadline (`core/crates/daemon/src/lifecycle/shutdown.rs`). Measured on the dev box: three sandboxes snapshotted in 0.75 s, the daemon exited in about a second, no Firecracker processes remained, and the snapshots resumed after a restart (`scripts/restart-test.sh`).

What survives a restart is what is durable by design: snapshots on disk (reconciled from the filesystem at startup, with their tap devices pulled back out of the free pool before the listener binds) and the history database. The history store exists to answer "did this exist and how did it end", which is a different problem from resuming it, and `EndReason::OrphanedByRestart` (`core/crates/store/src/lib.rs:53`) is how it records the sandboxes a previous daemon lost.

Sandbox identity is more than the VM. A sandbox's drive attachments, image, tap device and pool slot all have a "who holds this" check that spans live sandboxes and held snapshots, which is what prevents attaching one drive to two VMs or deleting an image mid-clone.

## Lifecycle states

The daemon models a sandbox's life as explicit transitions rather than flags.

```mermaid
stateDiagram-v2
    [*] --> Booting: POST /sandboxes
    Booting --> Running: Vm::boot ok, record_created
    Booting --> [*]: boot failed, lease and jail id released
    Running --> Snapshotted: snapshot_and_stop, also at SIGTERM shutdown
    Running --> Destroyed: destroy, keep=false
    Running --> Orphaned: daemon SIGKILL or crash
    Snapshotted --> Running: resume or fork
    Snapshotted --> Destroyed: snapshot deleted
    Destroyed --> [*]
    Orphaned --> [*]
```

`Running` is the only state with a live `Sandbox` in the daemon map. `Snapshotted` is a directory with three files (guest memory, device state, metadata) plus a held tap lease. `Orphaned` exists only in the history table, as an end reason. The default stop path is persistence-first: `stop_sandbox_by_id` (`core/crates/daemon/src/http/routes/sandbox.rs:669`) snapshots and then stops, and orderly shutdown reuses it for every live sandbox, and degrades to destroy for the two cases where there is nothing correct to preserve (a fork does not own its rootfs or lease outright, and a jailed sandbox cannot yet be snapshotted). See the [snapshot, resume and fork page](https://sandkiln.mintlify.site/internals/snapshot-resume-fork) for the persistence model.

## Concurrency model

Three rules cover the whole daemon.

- **Live state is behind `std::sync::Mutex`, nothing `.await`s while holding it, and a long call never holds it.** The sandbox map is small and its critical sections are short, so a plain mutex beats an async lock's subtleties. The rule that matters most is the last one: a user command can run for minutes, so `call_agent` copies an owned `AgentEndpoint` out of the map and drops the lock before calling. An earlier version held the lock across the whole vsock call and one slow command froze every other sandbox operation; `scripts/integration-tests/25-long-running-commands.sh` guards against that regression.
- **Correlation survives thread hops.** `tracing`'s current span is thread-local, so a bare `spawn_blocking` drops request correlation. The daemon wraps it in a helper that re-establishes the span and dispatcher on the blocking-pool thread, which is how one request id appears in log lines emitted from deep inside `vmm`.
- **Per-name serialisation, not global.** Concurrent claims of one sandbox name serialise on a lazily created per-name mutex, so unrelated names never wait on each other.

Background work (`lifecycle/idle_reaper.rs`, `lifecycle/pool_replenisher.rs`) runs as unconditional Tokio tasks that are cheap no-ops when nothing is configured.

## The case for a rootfs boundary

One boundary was drawn after measurement rather than by design instinct. A cold `POST /sandboxes` averaged 187.5 ms over 20 runs when the per-sandbox rootfs was a full byte copy on ext4 (rootfs clone 127.9 ms, VM boot 14.9 ms, network lease 3.5 ms). With the rootfs directory on the same XFS reflink filesystem as the base image, the same create averaged 17.9 ms and the clone took 0.08 ms. A later bench-report run (2026-11-23, 20 creates) measured a mean of 18.4 ms (clone 0.084 ms, lease 3.1 ms, boot 13.4 ms); both are directional figures from one shared box, and neither is authoritative. The clone therefore got its own module, `vmm::rootfs`, which issues the `FICLONE` ioctl and falls back to a byte copy only on `EOPNOTSUPP`, `EXDEV`, `EINVAL`, `ENOTTY` or `ENOSYS`:

```rust
let rc = unsafe { libc::ioctl(dst.as_raw_fd(), FICLONE, src.as_raw_fd()) };
if rc == 0 {
    return Ok(CloneMethod::Reflink);
}
```

(`core/crates/vmm/src/rootfs.rs:56-59`.) The clone method is returned, not hidden, and surfaces as `rootfs_clone_total{method="reflink"|"copy"}` on `/metrics`. A silent fallback to a copy looks identical to a slow disk in latency numbers, so the type makes the distinction observable. Two settings place the data: `rootfs_dir` for the per-sandbox rootfs files (it must share a filesystem with the base image for a reflink to be possible) and `state_dir` for durable daemon state. The [create path post](/engineering/04-create-path-and-rootfs) covers the clone and how a failed create unwinds it.

## Configuration

Configuration is one registry. `SETTINGS` in `core/crates/daemon/src/config/settings.rs` declares every setting once (31 of them: key, default, group, description), and everything else derives from it: the resolver, the `--config-reference` table, the example file and the environment variable names. A value resolves in a fixed order, environment (`SANDKILN_<KEY>`) over `sandkiln.toml` over the built-in default (`Resolver` in `config/source.rs`). All problems are accumulated into one `ConfigError` rather than panicking on the first, unknown TOML keys are rejected with a did-you-mean suggestion, and an `auth_token` in a group- or world-readable file produces a warning.

The file is found from `--config <file>`, then `$SANDKILN_CONFIG`, then `./sandkiln.toml`, then `~/.config/sandkiln/sandkiln.toml`. `sandkilnd` also takes `--check-config`, `--print-config`, `--config-reference`, `--help` and `--version` (`config/cli.rs`); the informational ones run before the daemon touches capabilities, storage or the network. `sandkiln.example.toml` at the repository root is guarded by a unit test so it cannot drift from the registry. `scripts/setup.sh` writes a `sandkiln.toml`, and `scripts/sandkilnd-ctl.sh` and `scripts/preflight-check.sh` read the effective values through `scripts/lib/daemon-config.sh`, which calls `sandkilnd --print-config`, and the ctl script runs `--check-config` before it starts the daemon. The settings that shape the architecture are `base_rootfs`, `kernel_path`, `firecracker_bin`, `rootfs_dir`, `state_dir`, `drives_dir`, `images_dir`, `history_db_path`, `auth_token`, `uplink_iface`, `exec_timeout_secs` and `jailer_enabled` (a daemon-operator switch, not a per-request override).

## Clients on top

The HTTP API is the only way in. The clients are deliberately thin.

- **`packages/sdk`** is the TypeScript client (`sandkiln` on npm). It wraps the HTTP API with no logic the daemon lacks, uses native `fetch`, and has zero runtime dependencies. For PTY and log streams it returns a native `WebSocket` rather than adding a `ws` dependency.
- **`packages/python`** mirrors the JS SDK operation for operation, with Python naming. It ships separate sync and asyncio class hierarchies on purpose, so a blocking call can never silently stall an event loop. It also has zero runtime dependencies.
- **`packages/cli`** is `kiln`, a `commander` wrapper over the JS SDK with essentially no logic of its own beyond argument parsing and output formatting.

The consequence for contributors is a rule from the repo's standard: a new daemon endpoint without an SDK method, CLI command and integration test is started, not done. Every layer a change touches is updated in the same change.

## Daemon source layout

`main.rs` is bootstrap only: parse flags, load configuration, raise the capability, build the runtime, wire the pieces together. The rest of `core/crates/daemon/src/` is split by concern:

```text
http/           mod.rs (build_router), auth.rs, error.rs, request_id.rs
  routes/       sandbox, sandbox_name, exec, fs, logs, mounts, tunnel, pty,
                preview, drives, images, pool, snapshot, snapshot_history, metrics
lifecycle/      idle_reaper, pool, pool_claim, pool_replenisher, shutdown
snapshots/      held.rs (snapshots a sandbox can resume from), retired.rs (checkpoints)
config/         mod.rs, settings.rs (the registry), source.rs (resolver), cli.rs
state.rs        AppState: live maps and the "who holds this" checks
storage.rs      StorageLayout and the OwnedFile scope guard
metrics.rs, sandbox.rs, log_session.rs, tracing_util.rs
```

## Repository layout

```text
core/crates/protocol/    wire format shared by host and guest
core/crates/guest-agent/ static musl binary, runs inside the VM
core/crates/vmm/         drives Firecracker + networking (host side)
core/crates/store/       durable sandbox-lifecycle history (sqlite)
core/crates/daemon/      axum HTTP API (sandkilnd)
packages/sdk/            sandkiln npm package (TypeScript)
packages/python/         sandkiln PyPI package (Python)
packages/cli/            kiln CLI, wraps the JS/TS SDK
images/                  rootfs/kernel build + agent-injection scripts
scripts/                 dev-box bootstrap, daemon lifecycle, tests
website/                 marketing and engineering pages (Astro)
docs-site/               full docs (Mintlify)
examples/                runnable reference projects
```

Testing follows the same layering. `cargo test --workspace` covers pure logic with no KVM (framing, jailer argument building, egress rule generation, the connect retry loop, the constant-time token comparison, `OwnedFile`, the configuration resolver). `Cargo.lock` is committed, so builds are reproducible. `scripts/integration-test.sh` exercises the daemon API against a real `sandkilnd`, with topic files under `scripts/integration-tests/` (about 320 checks across 26 topic files, numbered 00 to 25, the newest being long-running commands) that compare exact id sets rather than global counts so they can run concurrently. Anything touching KVM, tap devices or iptables is verified on the remote dev box, because a local build cannot prove it.

Known limitation: in the full four-way-parallel integration run, 2 or 3 of the roughly 320 checks fail because a freshly resumed guest kernel panics ("Attempted to kill the idle task!", about 20 ms after restore) and its Firecracker exits. 160 of 160 resume and re-snapshot cycles were clean in isolated and four-way-parallel loops, so it needs a specific concurrent condition; the root cause has not been found. Pool claims catch it with a post-resume health check.

## Reading further

- [Vsock protocol and the guest agent](/engineering/02-vsock-protocol-and-guest-agent): framing, message shapes, the reply bounds, the retry rule, and the hangup rules.
- [Driving Firecracker](/engineering/03-driving-firecracker): the boot sequence, jailer, and stop paths.
- [The create path and the rootfs clone](/engineering/04-create-path-and-rootfs): the cold create and how a failed one unwinds.
- Docs-site internals: [tap and bridge networking](https://sandkiln.mintlify.site/internals/tap-bridge-networking), [the sqlite history store](https://sandkiln.mintlify.site/internals/sqlite-history-store), [the jailer privilege model](https://sandkiln.mintlify.site/internals/jailer-privilege-model).

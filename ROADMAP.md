# Roadmap

This is a working plan, not a spec — it gets rewritten as we learn things.
Every milestone below ends with something concretely proven on real
hardware, not just code that compiles.

## What works today

- A Firecracker microVM boots under real KVM in ~30ms, with a guest agent
  running inside it that answers `exec` / `read_file` / `write_file` /
  `list_dir` over vsock.
- An HTTP daemon (`sandkilnd`) manages the full lifecycle — create, exec,
  list, stop — driving Firecracker directly from Rust rather than shelling
  out.
- Structured logging throughout: VM lifecycle events carry timing, HTTP
  requests are traced, everything is correlatable and filterable.
- Every sandbox gets real networking: its own tap device leased from a
  pool, attached to a shared bridge, its own IP, NAT'd outbound access,
  and DNS through a host-local proxy. Proven with two sandboxes running
  and reaching the internet concurrently through the daemon's HTTP API.
- A real JS/TS SDK — [`sandkiln` on npm](https://www.npmjs.com/package/sandkiln),
  published — `Sandbox.create()`, `runCommand()`, `stop()`, `Sandbox.list()`
  — verified end to end against the live daemon, not just typechecked in
  isolation.
- `criterion` benchmarks for boot time and exec latency, and a concurrent
  load-test script against the daemon's HTTP API.

## Engineering principles

- **Modular, not monolithic.** Each concern is its own crate/package with a
  narrow public API — `vmm`, `guest-agent`, the daemon, the SDK, the CLI
  stay separable. No crate reaches into another's internals.
- **Benchmark the hot paths.** Boot time, exec round-trip latency, and
  snapshot/resume time are the metrics that actually matter for this
  product. See the Benchmarking section below.
- **Prove it, don't assume it.** Every milestone ends with something
  actually run on real hardware, not just unit tests.
- **No half-finished surfaces.** A feature either works end to end or it
  isn't claimed as done. Deliberately deferred work is called out
  explicitly, not left silently incomplete.

Execution model: development happens in this repo; anything that needs
KVM, a Linux toolchain, or real hardware (Rust builds, Firecracker,
rootfs/kernel builds, actually booting a microVM) runs on the remote dev
box over SSH.

## Networking — done

Every sandbox leases a tap device from a pre-created pool
(`scripts/create-tap-pool.sh`) and attaches it to a shared bridge with a
statically assigned IP; the daemon runs unprivileged with `CAP_NET_ADMIN`
raised into its ambient set (`scripts/grant-net-admin.sh`), not as root.
Verified: two sandboxes running concurrently, each with a distinct IP,
both resolving DNS and reaching the real internet through the daemon's
HTTP API.

Sandboxes are also isolated from each other on the shared bridge (Linux
bridge port isolation — a tap can reach the gateway/uplink but not another
sandbox's tap), verified: cross-sandbox ping fails, gateway ping and real
outbound HTTP both still work.

## Client SDKs

- **JS/TS (`sandkiln` npm package) — working, matches the daemon's full
  surface.** `Sandbox.create()` (with tags and an auth token),
  `Sandbox.list()` (tag-filterable), `runCommand()`, `readFile()` /
  `writeFile()`, `stop()`. ESM + CJS + full type definitions via `tsup`.
  Verified against a live, auth-enabled daemon end to end — not just
  typechecked, which is how `stop()` returning `200` instead of the
  documented `204` got caught and fixed. **Published**:
  [npmjs.com/package/sandkiln](https://www.npmjs.com/package/sandkiln)
  (0.1.0, with signed provenance from the CI build). Still open: streamed
  logs, once the daemon can stream them.
- **Python (`sandkiln` PyPI package) — working, mirrors the JS SDK
  exactly.** `Sandbox.create()`/`attach()`/`list()`, `run_command()`,
  `read_file()`/`write_file()`, `stop()`. Zero runtime dependencies
  (stdlib `urllib`, matching the JS SDK's own zero-dependency `fetch`
  approach). Verified live end to end, including `attach()` reconstructing
  a handle without a network call and correct 404 handling on a stopped
  sandbox. Not published to PyPI yet.
- Both talk to the daemon's HTTP API — no logic duplicated between them
  beyond what each language's idioms require.

## CLI (`kiln`) — working

- **Done:** `kiln sandbox create|ls|rm|exec|read|write` — a thin
  `commander`-based wrapper over the SDK, verified live end to end.
  `cp` (a single unified copy command) was simplified to explicit
  `read`/`write` subcommands instead — less magic than parsing a
  `sandbox:path` prefix syntax for a first version.
- Still open: `kiln logs -f`, once the daemon can stream output.
- Built for manual testing, agentic workflows, and debugging — mirrors the
  SDK surface, usable standalone without writing code.

## Authentication and multi-tenancy

- **Done:** a single bearer token (`SANDKILN_AUTH_TOKEN`) gates every
  `/sandboxes*` route via daemon middleware; `/healthz` stays open. Off by
  default for local dev, with a startup warning so that's never silent.
  This is a self-hosted project, not tied to any platform's identity
  system, so a plain shared-secret token stands in for what a hosted
  platform would do with OIDC.
- Per-token scoping (which sandboxes a token can see/act on) once more than
  one caller shares a daemon instance.

## Base and custom images

- A **universal base image**: Ubuntu with current Node.js LTS, Python,
  common CLI tooling, and full root access inside the sandbox — the
  default every sandbox boots from unless told otherwise.
- A small catalog of **managed images** for common language runtimes.
- **Custom images**: accept a user-provided rootfs (or convert an OCI
  image into one) so teams can bake their own tooling in and reuse it
  across sandboxes.
- Image build tooling lives in `images/` — reproducible, scripted builds,
  not hand-built blobs.

## Persistence and snapshotting

- **Sandbox vs. session**: a sandbox is a persistent identity (name,
  config, filesystem state); a session is one running microVM instance of
  it. A sandbox resumed daily for a week is one sandbox, seven sessions —
  our current `Sandbox` type conflates the two (it dies with its VM) and
  needs to split before persistence can work at all.
- **Snapshot/resume**: save a running microVM's full state (memory + disk)
  and resume it later, skipping boot and dependency installation entirely.
- **Persistent-by-default sandboxes**: auto-snapshot on stop, so "stop and
  come back later" is the default behavior, not something the caller has
  to manage.
- **Named sandboxes**: create/resume by a caller-given name (unique per
  daemon) instead of only an opaque id, so `get-or-create` is possible
  without the caller tracking ids itself.
- Explicit snapshot API for the SDK/CLI to trigger a save point on demand.
- **Auto-suspend on idle**: pause (not destroy) a sandbox that's gone
  quiet for a configurable window, freeing its VM/network resources while
  keeping its state resumable — cheaper than staying booted, faster than
  a cold boot from scratch.
- **VM forking**: clone a *running* VM's memory and filesystem to start
  new sandboxes from that exact live state — distinct from snapshot/resume
  (which restarts from a saved point after a stop); this forks something
  still running, useful for parallel branches off one prepared environment
  (e.g. N parallel test runs off one dependency-installed base) without
  paying setup cost N times.
- **Time-travel restore**: keep more than just the latest snapshot per
  sandbox, so a caller can restore to an earlier point, not only the most
  recent stop.

## Drives and remote storage

- **Drives**: attachable persistent filesystem storage that outlives a
  single sandbox and can be reattached to a new one — for state that
  should survive well past any one VM's lifetime.
- **Read-only shared drives**: one drive mounted read-only across many
  sandboxes at once, for data or a common base layer that doesn't need
  per-sandbox copies — distinct from a per-sandbox writable drive.
- **Remote storage mounts**: mount an external object store (S3-compatible)
  into a sandbox via FUSE, so a sandbox can read/write remote files through
  its normal filesystem interface.

## Firewall and egress policy

- A per-sandbox network policy: default-open outbound (today's behavior)
  moving to an explicit allow/deny rule set the caller can configure —
  domains, IP ranges, ports.
- The DNS proxy (`start-dns-proxy.sh`) is the natural enforcement point
  for domain-level rules — it already sees every name a sandbox resolves,
  before any connection is made.
- Consider a per-sandbox CA + TLS-terminating proxy for HTTPS
  inspection/transformation, mounted into the guest's trust store at
  boot — meaningfully more complex than DNS-level filtering, so it's a
  deliberate stretch goal, not a given.

## Security hardening

- Firecracker's jailer: chroot, cgroups v2 resource limits, seccomp
  filters, dropped capabilities, an unprivileged uid per VM.
- Enforced per-sandbox resource ceilings (CPU, memory, disk) and an
  automatic idle timeout — today `vcpu_count`/`mem_size_mib` are set at
  boot but nothing stops a sandbox from running forever or a caller from
  requesting unreasonable resources.
- **Done:** network isolation between sandboxes on the shared bridge (see
  the Networking section) — bridge port isolation, no sandbox-to-sandbox
  traffic by default.

## Multi-agent isolation

- Separate Linux users with private home directories within a single
  sandbox, so multiple AI agents can share one VM without stepping on each
  other.
- Shared groups for deliberate, controlled file sharing between agents in
  the same sandbox.

## System-privileged workloads

- Support workloads that need real system-level privileges inside the
  guest: container runtimes (Docker-in-VM via nested virtualization or a
  compatible runtime), VPN clients, FUSE filesystem drivers.
- This needs care in the base image (kernel config, cgroups setup inside
  the guest) more than in the host-side daemon.

## Dev servers and live preview

- A host-side reverse proxy that maps a sandbox's port to a reachable URL,
  so a dev server running inside a sandbox can be previewed live.
- Fast iterative file sync tuned for dev-server workflows — write many
  small files quickly, ideally with watch-mode support.
- **Interactive terminal access**: a real PTY inside the sandbox, exposed
  over a WebSocket — distinct from batch `exec` (request in, response
  out); this is a live, bidirectional shell session, what `kiln`'s
  eventual interactive mode and any web-based terminal UI would need.

## Tags and sandbox metadata

- Key/value tags on sandboxes (environment, team, owner, whatever the
  caller wants) for filtering and listing.
- Sqlite-backed sandbox state instead of the current in-memory map, so
  tags, history, and listings survive a daemon restart.

## Benchmarking

- **Done:** `criterion` benchmarks in `sandkiln-vmm` for boot time and exec
  latency, run against the real Firecracker binary (not mocked). Run with
  `SANDKILN_BENCH_FIRECRACKER_BIN=<path> SANDKILN_BENCH_KERNEL_PATH=<path>
  SANDKILN_BENCH_ROOTFS_PATH=<path> cargo bench -p sandkiln-vmm --bench
  vm_lifecycle`.
- **Done:** A scripted load test: concurrent sandbox creation/exec through
  the daemon's HTTP API. Run with `scripts/load-test.sh [concurrency]
  [iterations] [base-url]` against a running `sandkilnd` (defaults: 10
  workers, 20 iterations each, `http://127.0.0.1:7777`).
- **Done:** A full end-to-end integration test, `scripts/integration-test.sh
  [base-url]` — sandbox lifecycle, tags, drives (including persistence
  across sandboxes and conflict detection), snapshot/resume, auth,
  `/metrics`, and error cases, all in one repeatable run against a real
  daemon. Tracks and tears down everything it creates. See root
  `AGENTS.md`'s "Integration testing" section.
- **Real measured results** (dev box, single node, 8-tap pool):
  - Cold boot (criterion): **32.3–33.1ms**.
  - Exec round-trip on an already-open vsock connection (criterion):
    **225–275µs**.
  - Load test, 4 concurrent workers × 5 cycles, 0 errors:
    **before** the fix below — 5.59 cycles/sec, `create` mean 369ms
    (p95 588ms).
    **after** — 5.38 cycles/sec, `create` mean **211ms** (p95 577ms),
    `exec` mean 366ms, `delete` mean 131ms. (Overall throughput is flat —
    `create` got cheaper but isn't the only phase in a cycle, and both
    runs are small samples on a shared, variable-load dev box — but the
    `create`-specific improvement is real and repeatable.)
  - **Finding, partially fixed**: `create`'s mean was far above the ~33ms
    cold-boot number — the gap was the ~300MB rootfs copy, done
    synchronously *after* the network lease. Fixed: the copy now runs
    concurrently with the lease (independent work, no reason to serialize
    them) and uses `cp --reflink=auto`, an instant copy-on-write clone on
    a filesystem that supports it (XFS, Btrfs). On this dev box's ext4,
    `--reflink` can't help — ext4 has no CoW — so the *remaining* gap
    (~180ms of real file-copy time) still needs either a CoW-capable
    filesystem for image storage or a device-mapper/thin-provisioning
    layer (ties into "Base and custom images").
- Snapshot/resume timing once that exists — the whole point of persistence
  is that resume should be dramatically faster than a cold boot; that
  claim needs a number behind it.
- These numbers are from one manual run on one shared dev box, not
  isolated hardware — treat them as directionally useful, not authoritative.
  Automating re-runs so regressions are visible over time is still open.

## Observability

- **Structured logging — working today.** `tracing` throughout, not just
  at the daemon's edge: HTTP requests/responses (method, path, status,
  latency) via `tower-http`'s `TraceLayer`, and VM lifecycle events (boot
  with timing, vsock call latency, stop) emitted from `sandkiln-vmm`
  itself so the library is useful standalone, not just under the daemon.
  Correlated by `vm_id`, filterable per-module via `RUST_LOG`.
- **Still open:**
  - Correlate an HTTP request all the way through to the VM operations it
    triggers with a single request/trace ID (currently `vm_id` and the
    axum request span are independent; needs a `tracing::Span` threaded
    through `Vm::boot`/`call`/`stop`).
  - A `/metrics` endpoint (Prometheus text format): sandboxes created
    (counter), sandboxes active (gauge), boot duration and exec latency
    (histograms).
  - JSON log output for production use (current output is pretty-printed
    for a human terminal) — switched by an env var.
  - Guest-side observability: today, output from a guest that fails before
    it can answer over vsock (kernel panic, agent crash) is invisible from
    the host. Worth capturing the serial console log per-VM regardless of
    whether vsock ever came up.

## Multi-node and regions

Everything so far assumes one daemon on one box. A single machine has a
ceiling — on concurrent sandboxes, on blast radius if it goes down, on
being close to wherever the caller actually is:

- Multiple daemon instances, each owning its own bridge/tap pool/rootfs
  storage, with something in front that knows which sandboxes live where
  (a routing layer, not full clustering — a sandbox is tied to the node
  it booted on, not migrated between them).
- A place identifier ("region") a caller can request at creation time,
  even if early on that just means "which physical box," not literal
  geographic distribution.
- This is deliberately last among the infrastructure work — it multiplies
  the surface area of everything above it (networking, images, storage),
  so it should land once those are individually solid on one node.

## Ecosystem and integrations

The primitive is only as useful as what's built on top of it:

- Example integrations with agent frameworks and coding-agent tools —
  showing a sandbox as the execution backend for agent-generated code,
  not just a standalone API.
- **Done:** a minimal reference "code playground"
  (`examples/code-playground`, JS/TS) and a reference "AI agent runner"
  (`examples/agent-runner`, Python) as real, runnable example projects
  against each SDK's actual current API — see `examples/AGENTS.md`.
- Consider what a plugin/adapter surface would look like once there's
  more than one real integration to generalize from — not before.
- **Deliberately out of scope for this project**: a durable-workflow layer
  (queues, timers, retries, fan-out across many sandboxes) is a different
  abstraction *on top of* a sandbox primitive, not part of the primitive
  itself. Worth knowing that shape exists — it's what turns "run this
  command in an isolated VM" into "orchestrate a long-running agent
  workflow" — but it belongs in a separate project/library built against
  this one's API, not merged into the daemon.

## Documentation and examples

- A docs site with runnable examples covering both SDKs and the CLI.
- Example projects: **done** — code playground (JS/TS) and AI-agent
  sandbox runner (Python), see `examples/`. Still open: a dev-server
  preview tool, once the reverse-proxy work in "Dev servers and live
  preview" above exists to build it on.

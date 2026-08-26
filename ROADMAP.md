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
- A real JS/TS SDK (`sandkiln` on npm, not yet published) — `Sandbox.create()`,
  `runCommand()`, `stop()`, `Sandbox.list()` — verified end to end against
  the live daemon, not just typechecked in isolation.
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

Known limitation worth its own follow-up: no isolation *between* sandboxes
on the shared bridge yet (any sandbox can currently reach another's IP) —
that's covered under Security hardening below, not solved here.

## Client SDKs

- **JS/TS (`sandkiln` npm package) — working.** `Sandbox.create()`,
  `sandbox.runCommand()`, `sandbox.stop()`, `Sandbox.list()`. ESM + CJS +
  full type definitions via `tsup`. Verified against the live daemon, not
  just typechecked — that's how `stop()` returning a 200 instead of the
  documented 204 got caught and fixed. Not published yet. Still open:
  `readFile()`/`writeFiles()` and streamed logs, once the daemon exposes
  them.
- **Python (`sandkiln` PyPI package).** Not started. Mirrors the JS SDK's
  surface and ergonomics once it exists.
- Both talk to the daemon's HTTP API — no logic duplicated between them
  beyond what each language's idioms require.

## CLI (`kiln`)

- `kiln sandbox create|exec|ls|rm|cp`, `kiln logs -f`.
- Built for manual testing, agentic workflows, and debugging — mirrors the
  SDK surface, usable standalone without writing code.

## Authentication and multi-tenancy

- Token-based auth on the daemon's HTTP API — this is a self-hosted
  project, not tied to any platform's identity system, so a straightforward
  bearer-token scheme (issued and checked by the daemon itself) replaces
  what a hosted platform would do with OIDC.
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

## Drives and remote storage

- **Drives**: attachable persistent filesystem storage that outlives a
  single sandbox and can be reattached to a new one — for state that
  should survive well past any one VM's lifetime.
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
- Network isolation *between* sandboxes on the shared bridge (see the
  Networking section) — no sandbox-to-sandbox traffic by default.

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
- **Real measured results** (dev box, single node, 8-tap pool):
  - Cold boot (criterion): **32.3–33.1ms**.
  - Exec round-trip on an already-open vsock connection (criterion):
    **225–275µs**.
  - Load test, 4 concurrent workers × 5 cycles, 0 errors, 5.59 cycles/sec:
    `create` mean 369ms (p95 588ms), `exec` mean 202ms (p95 453ms —
    inflated by cold execs hitting the agent-not-ready retry loop),
    `delete` mean 107ms (p95 126ms).
  - **Finding**: `create`'s ~369ms mean is far above the ~33ms cold-boot
    number — the gap is the ~300MB rootfs file copy done synchronously
    per sandbox (`std::fs::copy` in `routes.rs`), not VM boot itself. The
    real optimization target for sandbox creation latency is avoiding
    that copy (copy-on-write via `reflink`/overlayfs — ties into the
    "Base and custom images" section), not the boot path, which is
    already fast.
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
- A minimal reference "code playground" and a reference "AI agent runner"
  as real, runnable example projects (ties into Documentation below), not
  just SDK snippets.
- Consider what a plugin/adapter surface would look like once there's
  more than one real integration to generalize from — not before.

## Documentation and examples

- A docs site with runnable examples covering both SDKs and the CLI.
- Example projects: a code playground, an AI-agent sandbox runner, a
  dev-server preview tool — real uses of the primitive, not toy snippets.

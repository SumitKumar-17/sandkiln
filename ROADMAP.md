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
- Networking (tap + NAT + DNS) is proven to work but not yet wired into
  the daemon per-sandbox — every VM the daemon boots today has no network.
  That's the most important open gap.

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

## Networking — next up

Every sandbox the daemon boots needs its own tap device and IP, not the
single shared static one from early testing. This blocks everything
downstream that needs a sandbox to actually reach the network.

- A per-sandbox tap device + IP allocator (a small subnet, one /30 or /29
  per sandbox, or one shared bridge with per-VM DHCP-like static leases).
- Wire the existing NAT/DNS-proxy setup to work with dynamically created
  tap devices instead of one fixed one.
- Concurrency: prove multiple sandboxes can run and reach the network
  simultaneously without interfering with each other.

## Client SDKs

- **JS/TS (`sandkiln` npm package).** `Sandbox.create()`,
  `sandbox.runCommand()`, `sandbox.readFile()` / `writeFiles()`, streamed
  logs, `sandbox.stop()`. Ships ESM + CJS + full type definitions.
- **Python (`sandkiln` PyPI package).** Mirrors the JS SDK's surface and
  ergonomics.
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

- **Snapshot/resume**: save a running microVM's full state (memory + disk)
  and resume it later, skipping boot and dependency installation entirely.
- **Persistent-by-default sandboxes**: auto-snapshot on stop, so "stop and
  come back later" is the default behavior, not something the caller has
  to manage.
- Explicit snapshot API for the SDK/CLI to trigger a save point on demand.

## Drives and remote storage

- **Drives**: attachable persistent filesystem storage that outlives a
  single sandbox and can be reattached to a new one — for state that
  should survive well past any one VM's lifetime.
- **Remote storage mounts**: mount an external object store (S3-compatible)
  into a sandbox via FUSE, so a sandbox can read/write remote files through
  its normal filesystem interface.

## Security hardening

- Firecracker's jailer: chroot, cgroups v2 resource limits, seccomp
  filters, dropped capabilities, an unprivileged uid per VM.
- Network policy: isolated tap per sandbox (ties into the Networking
  section above), egress allow-listing, no sandbox-to-sandbox traffic by
  default.
- The DNS proxy is a natural enforcement point for domain-level egress
  policy — extend it rather than bolting policy on elsewhere.

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

Numbers gathered so far, informally, from tracing spans on the dev box:
boot ≈30ms, vsock round-trip ≈1ms. These need to become real, repeatable
benchmarks, not just log lines from one manual run:

- `criterion` benchmarks in `sandkiln-vmm` for boot time and exec latency,
  run against the real Firecracker binary (not mocked).
- A scripted load test: concurrent sandbox creation/exec through the
  daemon's HTTP API, to find where throughput actually breaks down once
  per-sandbox networking exists.
- Snapshot/resume timing once that exists — the whole point of persistence
  is that resume should be dramatically faster than a cold boot; that
  claim needs a number behind it.
- Published, checked-in results (not just claims) that get re-run and
  updated as the system changes, so regressions are visible.

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

## Documentation and examples

- A docs site with runnable examples covering both SDKs and the CLI.
- Example projects: a code playground, an AI-agent sandbox runner, a
  dev-server preview tool — real uses of the primitive, not toy snippets.

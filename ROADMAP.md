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
- Snapshot/resume/fork, durable across a daemon restart; a host-side
  reverse proxy for previewing a dev server running inside a sandbox;
  per-sandbox resource overrides with enforced ceilings; request-id
  correlation and a `/metrics` endpoint; opt-in Firecracker jailer
  hardening. All exposed through both SDKs and the CLI, live-verified via
  `scripts/integration-test.sh` (89 checks, 0 failing).

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
  surface.** `Sandbox.create()` (tags, an auth token, and optional
  `vcpuCount`/`memSizeMib` overrides), `Sandbox.list()` (tag-filterable),
  `Sandbox.resume()`/`Sandbox.fork()` (static, boot from a snapshot),
  `runCommand()`, `readFile()`/`writeFile()`, `snapshot()`, `previewUrl()`,
  `stop()`. ESM + CJS + full type definitions via `tsup`. Verified against
  a live, auth-enabled daemon end to end — not just typechecked, which is
  how `stop()` returning `200` instead of the documented `204` got caught
  and fixed. **Published**:
  [npmjs.com/package/sandkiln](https://www.npmjs.com/package/sandkiln)
  (0.2.0, with signed provenance from the CI build — includes everything
  in this bullet). Still open: streamed logs, once the daemon can stream
  them.
- **Python (`sandkiln` PyPI package) — working, mirrors the JS SDK
  exactly**, including `resume()`/`fork()`/`snapshot()`/`preview_url()`
  and resource overrides. Zero runtime dependencies (stdlib `urllib`,
  matching the JS SDK's own zero-dependency `fetch` approach). Verified
  live end to end, including `attach()` reconstructing a handle without a
  network call and correct 404 handling on a stopped sandbox. Not
  published to PyPI yet (see `packages/python/AGENTS.md`'s Publishing
  section — code-side ready, needs the account owner's one-time
  trusted-publisher registration).
- Both talk to the daemon's HTTP API — no logic duplicated between them
  beyond what each language's idioms require.

## CLI (`kiln`) — working

- **Done:** `kiln sandbox create|ls|rm|exec|read|write|preview|snapshot|
  resume|fork` — a thin `commander`-based wrapper over the SDK, verified
  live end to end. `cp` (a single unified copy command) was simplified to
  explicit `read`/`write` subcommands instead — less magic than parsing a
  `sandbox:path` prefix syntax for a first version.
- Still open: `kiln logs -f`, once the daemon can stream output.
- Built for manual testing, agentic workflows, and debugging — mirrors the
  SDK surface, usable standalone without writing code.
- **Published**: [npmjs.com/package/sandkiln-cli](https://www.npmjs.com/package/sandkiln-cli)
  (0.2.0, signed provenance) — not as `kiln`, which is a pre-existing,
  unrelated package (`node-kiln`, owned by someone else since before this
  project existed). `npm install -g sandkiln-cli` still gives you the
  `kiln` command (npm's `bin` field maps them independently). Verified
  live: installed fresh from the registry, ran `kiln --help` successfully.

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
- **Done (partial): custom/managed images.** `POST /images` registers an
  already-built ext4 rootfs from a host path into a daemon-managed
  directory (`SANDKILN_IMAGES_DIR`) under a caller-given id; `GET /images`
  lists them (`in_use_by`, always-`false` `guest_agent_verified` plus a
  `verification_hint`, since the unprivileged daemon can never loop-mount
  a candidate image to confirm the agent is baked in — use
  `scripts/preflight-check.sh --root-checks --rootfs-image <path>` out of
  band first); `DELETE /images/:id` refuses (409) while any live sandbox,
  in-flight boot, or held snapshot still references it. `POST /sandboxes`
  takes an optional `image_id` to boot from a registered image instead of
  `SANDKILN_BASE_ROOTFS`; carried through `snapshot`/`resume`/`fork` like
  `name`/`tags`. Exposed as `Image.register/list/delete` in both SDKs
  (plus `imageId`/`image_id` on `Sandbox.create`) and `kiln image
  ls|create|rm`, `kiln sandbox create --image`. **Not done:** OCI-image
  conversion — still accepts only an already-built ext4 file, not an OCI
  image reference — and there's no way yet to boot from an image by name
  through `get-or-create`.
- Image build tooling lives in `images/` — reproducible, scripted builds,
  not hand-built blobs.

## Persistence and snapshotting

- **Sandbox vs. session**: a sandbox is a persistent identity (name,
  config, filesystem state); a session is one running microVM instance of
  it. A sandbox resumed daily for a week is one sandbox, seven sessions —
  our current `Sandbox` type conflates the two (it dies with its VM) and
  needs to split before persistence can work at all.
- **Done: snapshot/resume.** Save a running microVM's full state (memory +
  disk) and resume it later, skipping boot and dependency installation
  entirely — `POST /sandboxes/:id/snapshot`, `POST /snapshots/:id/resume`,
  exposed as `Sandbox.snapshot()`/`Sandbox.resume()` in both SDKs and
  `kiln sandbox snapshot|resume`. Live-verified repeatedly this session,
  including with drives attached.
- **Done: snapshots durable across a daemon restart.** Snapshot metadata
  is written atomically to disk alongside its state/memory files and
  reconciled back into the daemon at startup — a snapshot taken before a
  daemon crash or restart is still listable and resumable afterward, with
  its held network tap device correctly reserved out of the pool before
  the daemon starts accepting new sandbox creates (preventing a
  double-lease race). Verified live: killed a daemon with a snapshot on
  disk, started a fresh instance, resumed it, data intact. Not durable
  across a host *reboot* by default — snapshot storage lives under
  `$TMPDIR`, see `SELF_HOSTING.md`'s persistent-state section.
- **Done: persistent-by-default sandboxes.** `DELETE /sandboxes/:id`
  auto-snapshots on stop by default (`stop_sandbox_by_id(..., keep: true)`
  via the shared `snapshot_and_stop`/`resume_snapshot_by_id` path in
  `routes_snapshot.rs`) instead of destroying — "stop and come back later"
  is now the default, not something the caller has to manage. An explicit
  `?keep=false` (CLI: `kiln sandbox rm --destroy`) opts back into a full
  destroy; a sandbox that structurally can't be preserved (jailed) falls
  back to destroy automatically rather than leaking. The idle reaper's
  destroy pass uses the same default.
- **Done: named sandboxes.** Create/resume by a caller-given name (unique
  among live sandboxes, `1-64` chars, `[A-Za-z0-9_-]`) instead of only an
  opaque id — `name` on `POST /sandboxes`, `GET /sandboxes/by-name/:name`
  (live only — 409 pointing at get-or-create if the name currently
  resolves to a held snapshot instead), and `POST /sandboxes/get-or-create`
  (return-if-live / resume-if-snapshotted / create-if-neither, race-safe
  under a per-name lock so two concurrent callers claiming the same new
  name can't both win). A name carries through `snapshot`/`resume`/`fork`
  when re-specified. Exposed as `Sandbox.getOrCreate()`/`Sandbox.byName()`
  in both SDKs (plus `name` on `create`/`resume`/`fork`) and
  `kiln sandbox get-or-create|get`, `--name` on `create`/`ls`/`resume`/
  `fork`.
- **Done: auto-suspend on idle.** `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`
  pauses and snapshots (not destroys) a sandbox that's gone quiet for a
  configurable window — the same pause+snapshot path the manual snapshot
  route uses, composed into the idle reaper rather than built as a new
  mechanism. Frees the VM/network resources it held while keeping state
  resumable; if it's also configured, `SANDKILN_IDLE_TIMEOUT_SECS` (plain
  destroy) must be strictly longer and acts as a backstop for a sandbox
  whose auto-suspend keeps failing, not a competing timer. Discoverability
  (a sandbox vanishing into a snapshot on its own): `GET /snapshots
  ?source_sandbox_id=<id>` finds what a given sandbox became —
  `Sandbox.listSnapshots()`/`list_snapshots()` in both SDKs,
  `kiln sandbox snapshots --source <id>`.
- **Done (partial): non-consuming snapshot fork.** `POST /snapshots/:id/fork`
  boots a new sandbox from a snapshot without consuming it, so the same
  prepared state can be resumed from repeatedly — `Sandbox.fork()` in both
  SDKs and `kiln sandbox fork`. **Not** true simultaneous parallel forking:
  Firecracker has no verified mechanism to give two live descendants of one
  snapshot independent rootfs backing files or independent guest IP/MAC
  (both are frozen into the snapshotted state), so at most one live fork of
  a given snapshot may exist at a time (`Snapshot::forked_into`, enforced
  for resume/fork/delete/snapshot alike — a second fork attempt while one
  is live is a `409`). Real parallel branches off one snapshot still needs
  either a verified per-fork drive-path override or a from-scratch
  live-memory-clone approach — genuinely open, see
  `core/crates/daemon/src/routes_snapshot.rs`'s module doc comment.
- **Time-travel restore**: keep more than just the latest snapshot per
  sandbox, so a caller can restore to an earlier point, not only the most
  recent stop.

## Drives and remote storage

- **Drives**: attachable persistent filesystem storage that outlives a
  single sandbox and can be reattached to a new one — for state that
  should survive well past any one VM's lifetime.
- **Done: read-only shared drives.** A drive attached read-only may be
  attached to arbitrarily many sandboxes at once — for data or a common
  base layer that doesn't need per-sandbox copies — while a read-write
  attachment (existing or requested) still needs exclusive, single-holder
  access, exactly like before this existed. `AppState::drive_holders()`
  tracks every current holder plus whether each holds it read-only;
  `can_attach_read_only()` is the pure rule deciding whether a new attach
  may coexist with what's already there. Covers snapshots holding a drive
  too, not just live sandboxes.
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

- **Done (opt-in): Firecracker's jailer** — chroot, cgroup v2 resource
  limits, a dedicated unprivileged uid/gid per VM
  (`SANDKILN_JAILER_ENABLED`, see `SELF_HOSTING.md`'s "Optional:
  jailer-based sandbox boot"). Off by default; the daemon still boots
  every sandbox via a direct Firecracker spawn unless explicitly turned
  on. Builds and passes unit tests, but the actual chroot/cgroup/uid-drop
  behavior against a real installed jailer binary hasn't been proven on
  real hardware yet — verify before relying on it for a genuinely
  adversarial workload. Snapshotting a jailed sandbox isn't supported
  (`400`) — jailer support covers `Vm::boot` only, `Vm::resume` always
  spawns directly.
- **Done: enforced per-sandbox resource ceilings.** A `POST /sandboxes`
  request may now override `vcpu_count`/`mem_size_mib` per sandbox
  (defaulting to the daemon's configured values when omitted), checked
  against `SANDKILN_MAX_VCPU_COUNT`/`SANDKILN_MAX_MEM_SIZE_MIB` — `0` or
  above-ceiling is rejected with `400`, not silently clamped. Live-verified.
  seccomp filters and disk-size ceilings are still open.
- **Done:** automatic idle timeout (`SANDKILN_IDLE_TIMEOUT_SECS`) — a
  sandbox with no exec/read/write activity past the configured window is
  stopped automatically.
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

- **Done: host-side reverse proxy.** `GET/POST/... /sandboxes/:id/preview/:port[/path]`
  proxies a full HTTP request to a server listening on that port inside
  the sandbox, over the bridge network — `Sandbox.previewUrl()`/
  `preview_url()` in both SDKs, `kiln sandbox preview`, an
  `examples/dev-server-preview` reference. Preview routes accept the auth
  token as a `?token=` query parameter (not just a header), since the
  caller is typically a browser tab or `<iframe>` that can't set one.
  Live-verified, including a real `python3 -m http.server` proxied
  end to end, the 502/404/401 error paths, and previewing a sandbox
  forked from a snapshot (which borrows its network lease rather than
  owning one). WebSocket proxying (dev-server HMR/live-reload) is a real,
  explicitly-scoped-out follow-up, not silently broken — plain HTTP only
  for now.
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
- **Done: request-id correlation.** Every HTTP request gets an id (caller-
  supplied via `X-Request-Id`, or generated) established as the active
  `tracing::Span` before any handler runs, echoed back in the response,
  and propagated across `spawn_blocking` into every `sandkiln-vmm` call
  the request triggers (`tracing_util::spawn_blocking_in_current_span`) —
  so one id ties an HTTP request to the VM boot/call/stop log lines it
  caused. Live-verified.
- **Done: `/metrics` endpoint** (Prometheus text format, unauthenticated
  like `/healthz`): `sandboxes_created_total` (counter), `sandboxes_active`
  (gauge), boot duration and exec latency (histograms). Hand-rolled text
  exposition rather than a new dependency — see `metrics.rs`.
- **Done: JSON log output** (`SANDKILN_LOG_FORMAT=json`) for production log
  pipelines, alongside the default pretty terminal format.
- **Done: guest-side console capture.** A guest that fails before it can
  answer over vsock (kernel panic, agent crash) is no longer invisible —
  the spawned Firecracker process's stdout/stderr (the guest's
  `console=ttyS0` output) is captured to a per-VM log file, and a boot
  failure's error message includes that file's path.

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
- Example projects: **done** — code playground (JS/TS), AI-agent sandbox
  runner (Python), and dev-server preview (JS/TS), see `examples/`.

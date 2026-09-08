# Roadmap

This is a working plan, not a spec — it gets rewritten as we learn things.
Every milestone below ends with something concretely proven on real
hardware, not just code that compiles.

## What works today

- A Firecracker microVM boots under real KVM in ~30ms, with a guest agent
  running inside it that answers `exec` / `read_file` / `write_file` /
  `list_dir` over vsock.
- **Current sandbox launch latency, measured, not estimated**: a full
  `POST /sandboxes` create averages **211ms** (min 137ms, p95 577ms,
  under concurrent load) — the ~30ms boot time above plus rootfs-copy
  and network-lease overhead. The ~180ms gap between raw boot and full
  create is the known, actively-tracked bottleneck (see Benchmarking and
  Persistence and snapshotting below for the CoW-filesystem and
  pre-warmed-pool plans to close it) — worth stating up front since it's
  the number that actually matters for "how long until I can run code,"
  not the boot time alone.
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
  `scripts/integration-test.sh` (154 checks, 0 failing).

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
  them — the shape worth copying when that's built is a replay-then-
  live-tail model (reconnecting gets everything since the process
  started, not just what's emitted from that point on), not just a bare
  live tail.
- **Filesystem operations are read-file/write-file only.** No
  `chmod`/`chown`/`symlink`/`readlink`/`mkdir`/`rename`/`copy`/
  `truncate`/directory-listing-with-metadata — a caller has to shell out
  via `runCommand` for anything beyond reading or overwriting one file's
  full contents. This is a developer-experience gap, not a capability
  one (the guest agent could expose these as additional vsock commands
  using the same protocol shape `read_file`/`write_file` already use),
  but a real one if a workload wants filesystem operations as first-class
  SDK calls instead of shelled-out commands.
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
- **Tiered idle lifecycle**: extend today's binary auto-suspend
  (running → snapshot) into named tiers with independently configurable
  windows — e.g. suspend past one timeout, then archive (move snapshot
  storage off hot local disk to cheaper/remote storage, ties into the
  Drives and remote storage section) past a longer one, then delete past
  a longer one still. Not started; auto-suspend's existing
  `snapshot_and_stop` path is the natural base to extend rather than a
  new mechanism.

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
- **Not yet done: drives in either SDK or the CLI.** Only the daemon's
  raw HTTP API supports attaching drives at create time today — neither
  the JS/TS nor the Python SDK exposes it on `Sandbox.create()`, and
  `kiln` has no `--drive`/`--drives` flag either.
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
- A richer shape worth designing toward from the start, since retrofitting
  precedence rules later is worse than deciding it up front: a base mode
  (allow-all/deny-all) plus a domain allowlist plus *separate* subnet
  allow and deny lists where deny takes precedence over allow on overlap
  — not just one flat allow list. Request-level matchers (path, method,
  query, header) with a rule that either forwards or transforms the
  request are a further-out stretch beyond that, useful for a proxy
  sitting in front of a sandbox's own exposed port rather than the
  sandbox's own outbound egress.
- Consider a per-sandbox CA + TLS-terminating proxy for HTTPS
  inspection/transformation, mounted into the guest's trust store at
  boot — meaningfully more complex than DNS-level filtering, so it's a
  deliberate stretch goal, not a given.

## Security hardening

- **Isolation model, stated explicitly**: every sandbox is a real
  Firecracker microVM with its own kernel, not a shared-kernel
  container/namespace sandbox with syscall interception. This is the
  strongest isolation story available for running untrusted code and is
  worth saying outright (in docs and on the website) rather than leaving
  it implicit — it's the actual reason this project exists in this shape
  rather than as a thinner container wrapper.
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
- **Done: per-sandbox I/O rate limiting.** `POST /sandboxes` (and
  `get-or-create`) takes an optional `rate_limit: {bandwidth_bytes_per_sec?,
  ops_per_sec?}`, validated by the same "reject, don't silently no-op"
  rule as `vcpu_count`/`mem_size_mib` (at least one sub-field required if
  present at all, `0` rejected outright with `400`) and applied uniformly
  to the rootfs drive, every attached drive, and both directions of the
  network interface via Firecracker's own token-bucket rate limiter
  (`rate_limiter` on each drive, `rx_rate_limiter`/`tx_rate_limiter` on
  the NIC — confirmed against the real `firecracker_spec` swagger schema,
  not assumed). Each bucket refills to its full size once per second, a
  deliberately simple sandbox-level knob rather than exposing
  Firecracker's own independent-burst/independent-direction granularity —
  that's still available at the vmm-crate level (`VmConfig::rate_limit`,
  `sandkiln_vmm::vm::{RateLimiter, TokenBucket}`) if a future need for
  finer control shows up. `None` (the default) means unlimited host I/O,
  unchanged from before this existed. **Not yet done:** exposed in either
  SDK or the CLI — daemon-level only for now, same shape as the
  drives-in-SDK gap below.

## Multi-agent isolation

- **Done: separate Linux users with private home directories, baked
  into the base image.** `images/setup-multi-agent-users.sh` creates
  per-agent users (private `$HOME`) plus a shared group for deliberate,
  controlled file sharing between agents in the same sandbox — run as
  part of `images/build-universal-image.sh`, not something a caller
  configures at create time.
- Runtime-configurable agent users (chosen per sandbox at `POST
  /sandboxes` time, rather than fixed at image-build time) is still
  open.

## System-privileged workloads

- Support workloads that need real system-level privileges inside the
  guest: container runtimes (Docker-in-VM via nested virtualization or a
  compatible runtime), VPN clients, FUSE filesystem drivers.
- This needs care in the base image (kernel config, cgroups setup inside
  the guest) more than in the host-side daemon.
- **GPU passthrough/access inside a sandbox is explicitly not planned.**
  Called out here deliberately rather than left silent — Firecracker
  itself has no GPU device model, and adding one would be a different
  project (a real hardware-passthrough VMM), not an extension of this
  one. If a workload needs a GPU, it doesn't belong in a sandkiln
  sandbox today.

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
- **Alternative worth considering alongside the proxy**: today's
  `/preview/:port` is a daemon-proxied *path*, not a real routable
  domain. A dedicated public domain/subdomain per exposed port (the
  guest reachable at its own URL rather than through a `/preview/:port`
  path prefix) is a different, also-valid shape used elsewhere — would
  need real DNS/routing infrastructure this project doesn't have today,
  so it's a bigger lift than the existing proxy, not a drop-in
  replacement for it.
- Fast iterative file sync tuned for dev-server workflows — write many
  small files quickly, ideally with watch-mode support.
- **Interactive terminal access**: a real PTY inside the sandbox, exposed
  over a WebSocket — distinct from batch `exec` (request in, response
  out); this is a live, bidirectional shell session, what `kiln`'s
  eventual interactive mode and any web-based terminal UI would need.
  Validated as a real, commonly-offered capability elsewhere, not a
  fringe idea — not currently in active development. If/when this is
  built, a per-sandbox concurrent-session cap (a fixed ceiling like 64)
  is a sane default worth copying rather than leaving unbounded.

## Tags and sandbox metadata

- Key/value tags on sandboxes (environment, team, owner, whatever the
  caller wants) for filtering and listing.
- Sqlite-backed sandbox state instead of the current in-memory map, so
  tags, history, and listings survive a daemon restart.
- **Guest-accessible metadata service**: sandkiln has no way for code
  running *inside* a sandbox to read its own tags/name/config from the
  daemon — a caller has to bake anything the guest needs to know about
  itself into the rootfs at image-build time, or push it in after boot
  via `write_file`. Firecracker itself documents exactly this gap being
  solvable: a host-configurable, guest-reachable metadata endpoint the
  guest can query over its own network path, updatable by the host
  without a reboot. Not started, but worth designing toward — it's the
  natural place tags/name would become visible to the workload running
  inside the sandbox itself, not just to the caller managing it from
  outside.

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
- **Done: snapshot/resume benchmarked** (`bench_snapshot_take`/
  `bench_resume` in `core/crates/vmm/benches/vm_lifecycle.rs`, alongside
  the existing `bench_cold_boot`/`bench_exec_roundtrip`). Real numbers,
  same dev box as above:
  - `snapshot_take` (pause + write memory/state to disk): **~322ms**
    (309.6–335.0ms) — expensive, dominated by writing the guest's full
    memory to disk synchronously; a real cost for auto-suspend and any
    future tiered-idle-lifecycle work, not free.
  - `resume_from_snapshot`: **~25.8ms** (25.5–26.2ms) — only **~19%
    faster than `cold_boot`'s ~31.9ms** (31.5–32.4ms), not the dramatic
    win the earlier framing below assumed. Both numbers are already
    small on this base image (lightweight kernel, minimal guest-agent
    init) — cold boot has little slack left for resume to undercut.
  - **This changes what a pre-warmed pool actually buys**: the ~180ms
    gap between a ~32ms boot and the measured 211ms full-create (see
    "What works today" above) isn't in the boot/resume step at all —
    both are ~25–32ms either way — it's in the surrounding per-create
    setup (rootfs prep, network lease). A pre-warmed pool's real value
    is pre-doing *that* setup ahead of a request, not shaving boot
    latency itself, which was never the bottleneck. Worth re-measuring
    once a pool exists, rather than assumed up front.
- **Pre-warmed snapshot pool**: the mechanism sandkiln already has
  (snapshot/resume, auto-suspend) is the same one production Firecracker
  users document as their main cold-start fix — restore an
  already-initialized VM instead of booting one from scratch. The gap:
  sandkiln only takes that path reactively (idle-timeout-triggered or a
  caller's own explicit `snapshot()`), never proactively ahead of a
  request the way a pre-warmed pool would. Concrete next step: keep a
  small pool of ready-to-resume snapshots (per image/config) so a
  `get-or-create`/create-from-image call can resume one instead of
  cold-booting — now scoped correctly per the finding above: the win is
  skipping per-create rootfs/network setup ahead of time, with the
  boot-vs-resume gap itself a secondary, much smaller effect. Shape
  worth copying from how other pooled-sandbox systems configure this,
  if/when it's built: pool identity keyed by image+resource-config (not
  just image), a configurable warm-instance count with `0` meaning
  scale-to-zero, a separate max-instance ceiling above the warm count,
  queueing (not rejecting) a claim that arrives at the ceiling, and the
  pool auto-replacing a claimed instance in the background to keep the
  warm buffer full rather than refilling only on the next claim.
- **Snapshot lineage**: today a daemon only ever knows "the current
  snapshot" a sandbox became — there's no way to ask "what snapshot did
  *this* snapshot get forked from, and what else was forked from it."
  Walking that ancestry (a tree, not just a single pointer) would need
  `Snapshot` to record its own parent snapshot id when created via
  `fork`/`resume`, not just `Sandbox::source_snapshot_id`'s current
  one-hop pointer. Not started.
- **Fan-out cloning**: cloning one snapshot into *several* new live
  sandboxes at once (not just one) is a real, documented pattern
  elsewhere, but it directly conflicts with the one-live-fork-per-snapshot
  constraint above (`Snapshot::forked_into`) — that constraint exists
  because Firecracker has no verified way to give two live descendants
  of one snapshot independent rootfs backing files or guest IP/MAC (see
  the fork bullet above). Genuine fan-out would need that same unsolved
  problem solved first, not a new API layered on top of today's fork;
  tracked here as a restatement of the existing gap in fan-out terms,
  not a separate, independently-achievable item.
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

## Ideas explicitly not started, kept honest rather than silent

Two capabilities other sandbox platforms document that sandkiln has
zero coverage of — each closer to a separate product surface than a
small extension of what exists, called out deliberately (same reasoning
as the GPU-passthrough note above) rather than left as a silent gap:

- **Desktop/GUI automation**: a managed desktop environment inside a
  sandbox (a windowing environment plus a browser, reachable over VNC or
  a browser-based remote-desktop client), with a screenshot-capture API
  and a full keyboard/mouse input-control API, plus reconnecting to an
  already-running desktop sandbox by id without killing the VM. Real,
  documented use cases: browser-based agent automation, computer-use
  agent evals. This is a meaningfully different product surface than
  "run untrusted code and read/write files" — it needs its own base
  image work (a desktop environment, a VNC server, input-injection
  tooling inside the guest) well beyond today's universal image.
- **Git-native sandbox filesystem**: version-controlled sandbox state as
  a first-class concept — workspaces as private branches mounted as
  POSIX directories, a snapshot doubling as a commit, merging by moving
  a pointer rather than copying data, and read-only mounts for sharing
  common tooling across many sandboxes either following a moving head or
  pinned to one snapshot. A substantial new subsystem (a real git
  server/object store this project doesn't have any of today), not a
  small extension of `snapshot`/`resume`/`fork` — those stay path-based
  and single-lineage; this would be a different persistence model
  layered alongside them, not a replacement.

Both are plausible future directions for this project specifically
because it's a personal project without a fixed roadmap deadline, not
because either is a small lift — treat this section as "worth doing
eventually if there's appetite," not "next."

## Documentation and examples

- **Done**: a full docs site (`website/docs/`, Astro + Starlight) —
  Getting Started, Core Concepts, Guides, Reference (daemon HTTP API,
  JS/TS SDK, Python SDK, CLI), and Architecture, covering both SDKs and
  the CLI with runnable examples throughout. Deployed three ways: as a
  `/docs` subpath of the main site (GitHub Pages and the merged Vercel
  build) and standalone at its own domain root
  (sandkiln-docs.vercel.app) — see `website/AGENTS.md`.
- Example projects: **done** — code playground (JS/TS), AI-agent sandbox
  runner (Python), and dev-server preview (JS/TS), see `examples/`.

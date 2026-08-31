# AGENTS.md — sandkiln-daemon

Read the root `AGENTS.md` first for project-wide conventions and the
gotchas list. This file is scoped to this one crate (`sandkilnd`, the
HTTP API).

## What this crate is

An axum + tokio HTTP server wrapping `sandkiln-vmm`'s VM lifecycle into a
REST-ish API. This is the thing SDKs and the CLI actually talk to. Keep
business logic (VM lifecycle, networking) in `sandkiln-vmm` — this crate
should mostly be: parse a request, call into `vmm`, shape a response.

## Files

- `main.rs` — **not** `#[tokio::main]`, deliberately (see the capability-
  ordering gotcha in root `AGENTS.md` — read it before touching this
  file's structure). Builds the router, wires auth middleware onto the
  `/sandboxes*` routes only (`/healthz` stays open), raises
  `CAP_NET_ADMIN` before the Tokio runtime starts.
- `config.rs` — `Config::from_env()`, every daemon env var
  (`SANDKILN_*`) in one place. Adding a new configurable thing means a
  new field here plus an `env_or`/parse call, following the existing
  pattern. Also defines `LogFormat` (`SANDKILN_LOG_FORMAT=json` vs. the
  default pretty output), read by `main.rs` before the tracing
  subscriber is initialized. `JailerHostConfig`/`Config::jailer`
  (`SANDKILN_JAILER_ENABLED` and friends) is the daemon-operator switch
  for jailer-based sandbox boot — see `sandkiln_vmm::jailer` and
  `SELF_HOSTING.md`'s jailer section. Deliberately not something a
  `POST /sandboxes` request body can override.
- `metrics.rs` — `Metrics`: the `/metrics` endpoint's counters/gauge/
  histograms and a hand-rolled Prometheus text-exposition-format writer.
  Lives on `AppState` (`state.metrics`); route handlers record into it at
  the same call sites `sandkiln-vmm`'s `tracing` events fire from
  (`routes_sandbox::create_sandbox` for boot duration and the created
  counter, `routes_exec::call_agent` for exec latency). No metrics crate
  dependency — see the module doc comment for why.
- `auth.rs` — bearer-token middleware. No-ops entirely if
  `SANDKILN_AUTH_TOKEN` is unset.
- `state.rs` — `AppState`: the daemon's config, `NetworkManager`, an
  optional `JailerIdPool` (`Some` only when `config.jailer` is set), and
  in-memory sandbox map (`Mutex<HashMap<String, Sandbox>>`). This map
  *is* the daemon's entire notion of sandbox state — it doesn't survive a
  restart (see `ROADMAP.md`'s sqlite-backed-state item). Also owns naming:
  `name_holder`/`resolve_name` find whichever of a live sandbox or a held
  snapshot currently carries a given name (live wins if both do — see
  `Sandbox::name`'s doc comment on why that's not a conflict), and
  `lock_name` hands out a per-name `tokio::sync::Mutex` (with best-effort
  cleanup once nothing references it) that every code path claiming or
  resolving a name serializes on, so two concurrent callers can't both
  win a race for the same brand-new name. Also owns `drives`/`images`
  (the `DriveStore`/`ImageStore` from `sandkiln-vmm`) and the
  ownership-tracking helpers that answer "who currently holds this
  resource" across live sandboxes and held snapshots in one place —
  `drive_holder()`/`image_holder()` — plus `reserve_pending_image_boot`/
  `release_pending_image_boot`, which extend that tracking to cover an
  image referenced by a boot that's still in flight (not yet a `Sandbox`
  in the map), closing the race where `DELETE /images/:id` could
  otherwise remove a file an in-progress rootfs copy is still reading.
- `sandbox.rs` — the `Sandbox` struct the daemon tracks per running VM
  (id, `Vm` handle, network `Lease`, rootfs path, tags, created-at,
  `last_activity`, `image_id` — the registered image this sandbox's
  rootfs was cloned from, if any, `None` meaning the daemon-wide
  `SANDKILN_BASE_ROOTFS` default — `jail_id`, the leased uid/gid if this
  sandbox booted jailed, released back to `state.jailer_ids` on stop —
  and `name`, the caller-given identity carried across the
  sandbox<->snapshot boundary).
- `routes_sandbox.rs` — sandbox lifecycle handlers: create/list/stop.
  `stop_sandbox_by_id()` is the shared stop entry point used by both the
  `DELETE` route and `idle_reaper`; it defaults to preserving state
  (snapshot-then-stop, via `routes_snapshot::snapshot_and_stop`) rather
  than destroying it, with `destroy_sandbox_by_id()` — the original
  teardown (VM stop, network release, rootfs cleanup) — reached via the
  `?keep=false` opt-out or as the correct silent fallback for a forked
  sandbox (nothing new to preserve) or a jailed one (can't be
  snapshotted, surfaces as an error instead of silently discarding
  state). `create_sandbox_core()` is the actual boot logic, shared with
  `routes_sandbox_name::get_or_create_sandbox`'s create-fresh path — the
  `create_sandbox` handler itself adds the name-uniqueness check under
  `AppState::lock_name` and resolves which rootfs to clone from
  (`state.config.base_rootfs_path` by default, or a registered image's
  path when the request gives an `image_id`, via `AppState::images`,
  reserving/releasing a pending-boot claim on that image id around the
  whole boot with `PendingImageBootGuard` so a concurrent image deletion
  can't race an in-flight clone) before calling it.
- `routes_sandbox_name.rs` — name-based lookup and get-or-create:
  `GET /sandboxes/by-name/:name` (live sandboxes only — a name currently
  held by a snapshot is a `409` pointing at get-or-create, not a silent
  resume) and `POST /sandboxes/get-or-create` (return-if-live /
  resume-if-snapshotted / create-if-neither, race-safe under
  `AppState::lock_name`). Split out from `routes_sandbox.rs` since it
  crosses into snapshot territory (`routes_snapshot::resume_snapshot_by_id`).
- `routes_images.rs` — registered-image handlers: `POST /images`
  (register an already-built ext4 rootfs from a host path, copying it
  into `SANDKILN_IMAGES_DIR` via `sandkiln_vmm::image::ImageStore`),
  `GET /images`, `DELETE /images/:id` (refuses via `AppState::image_holder`
  while any live sandbox, in-flight boot, or held snapshot references
  it — same pattern as `routes_drives::delete_drive`). Every response
  says `guest_agent_verified: false` — the daemon runs unprivileged and
  cannot loop-mount a candidate image to check the agent is baked in;
  `scripts/preflight-check.sh --root-checks --rootfs-image <path>` is
  the only way to get that confirmation, out of band, before registering.
- `routes_exec.rs` — exec/read-file/write-file handlers. `call_agent()`
  is the shared helper all three use — extend it, don't duplicate its
  pattern. It's also what bumps a sandbox's `last_activity`.
- `idle_reaper.rs` — background task (spawned from `main.rs` whenever
  `SANDKILN_IDLE_TIMEOUT_SECS` and/or `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`
  is set) that reclaims idle sandboxes two ways: auto-suspend (pause +
  snapshot, via `routes_snapshot::snapshot_and_stop`) past
  `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`, and destroy (via
  `routes_sandbox::stop_sandbox_by_id`, same preserve-by-default behavior
  as an explicit stop — see above — with a fallback to a real destroy only
  when preservation is structurally impossible, so an unpreservable idle
  sandbox doesn't leak forever) past `SANDKILN_IDLE_TIMEOUT_SECS`. Each
  tick runs auto-suspend first, then destroy against whatever's still
  running — see `config::Config::auto_suspend_timeout`'s doc comment for
  why `auto_suspend_timeout` is required to be strictly shorter than
  `idle_timeout` when both are set (destroy is a backstop for a
  persistently-failing auto-suspend, not a competing timer).
- `snapshot.rs` — the `Snapshot` type (`state.snapshots`'s value type)
  plus everything that makes it durable across a daemon restart: on-disk
  metadata (`meta.json`, alongside `state.snap`/`mem.bin` under
  `snapshot_dir(id)`) written atomically via write-then-rename, and
  `reconcile()`, which scans `snapshots_root()` at startup and rebuilds
  `AppState::snapshots` from what's actually on disk — the same
  "filesystem is the source of truth" pattern `sandkiln_vmm::drive`'s
  `DriveStore::list()` uses for drives. A snapshot directory missing any
  of its three files is treated as a crash-mid-write and skipped with a
  warning rather than guessed at. `reconcile()` also calls
  `NetworkManager::reserve()` for each reconciled snapshot's held tap
  device/host octet so a live `lease()` call afterward can't hand the
  same tap to a second sandbox — see `main.rs`, which runs this before
  the HTTP listener starts accepting connections.
- `routes_drives.rs` / `routes_snapshot.rs` — drives and snapshot/resume
  handlers, each in their own file for the same reason as above. The
  actual pause/snapshot/stop mechanics live in `snapshot_and_stop()` and
  the actual resume mechanics in `resume_snapshot_by_id()` — both
  `pub(crate)`, both reused by `routes_sandbox`'s persistent-by-default
  stop, `routes_sandbox_name`'s get-or-create, and `idle_reaper`'s
  auto-suspend so there's exactly one place that knows what "snapshot this
  sandbox" / "resume this snapshot" means. `check_snapshottable`/
  `SnapshotBlocked` refuses to snapshot a jailed sandbox (`Vm::is_jailed`)
  — `Vm::resume` only ever spawns directly, so a jailed sandbox's snapshot
  could never be resumed correctly; see `sandkiln_vmm::jailer`'s module doc
  comment before changing this — or a sandbox forked from another snapshot
  (shares its rootfs file, would corrupt on resume). `list_snapshots` takes
  an optional `?source_sandbox_id=` filter — how a caller looks up whether
  a sandbox id it had turned into a snapshot (via auto-suspend or a manual
  snapshot).
- `routes_metrics.rs` — the `/metrics` handler. Unauthenticated like
  `/healthz` (wired directly on `app` in `main.rs`, not through either
  auth-gated router) since it's operational data about the daemon, not
  sandbox data.
- `routes_preview.rs` — the `GET/POST/... /sandboxes/:id/preview/:port[/*path]`
  reverse proxy: forwards a full HTTP request to
  `http://<sandbox guest ip>:<port>/<path>` on the bridge network
  (`sandkiln_vmm::network::Lease::config.guest_ip`) via `AppState::preview_client`
  (a `hyper_util::client::legacy::Client`, built once in `state::build_preview_client`
  so requests reuse pooled connections), and streams the response straight
  back. Its own router in `main.rs`, guarded by `auth::require_preview_token`
  instead of `auth::require_bearer_token` — see that middleware's doc
  comment and this module's doc comment for the auth reasoning (short
  version: a browser navigating directly to a preview URL can't attach an
  `Authorization` header, so this route also accepts the token as a
  `?token=` query parameter, which is then stripped, along with the
  `Authorization` header itself, before anything is forwarded to the
  guest — the guest runs untrusted/AI-generated code and must never see
  this API's credential). Connection-refused/unreachable maps to
  `AppError::BadGateway` (502); no response within `Config::preview_timeout`
  maps to `AppError::GatewayTimeout` (504) — see `error.rs`.
- `error.rs` — `AppError`, the one error type every handler returns.
  Add a variant here rather than inventing a new ad hoc error shape.

## Building, running, and verifying

See root `AGENTS.md`'s full checklist (sync → build → clippy → grant
`CAP_NET_ADMIN` if rebuilt → run with real env vars → drive it with curl
or a real client → clean up). The short version specific to this crate:
`cargo build -p sandkiln-daemon` catches compile errors; nothing short of
actually starting `sandkilnd` and hitting its HTTP API proves a route
works. **Every route in this file was live-tested against a real running
daemon before being called done — do not skip that step because "it
typechecks."** The `DELETE` status-code bug (200 instead of documented
204) is the canonical example of a bug that compiled and clippy-passed
cleanly but was still wrong.

## Non-obvious things specific to this crate

- **New route handlers that touch the sandbox map or `vmm` should go in
  their own `routes_*.rs` file** — when multiple people (or parallel
  agents) are adding features concurrently, a shared file is a
  guaranteed merge-conflict point, and this is also why `routes.rs` got
  split into `routes_sandbox.rs`/`routes_exec.rs` once it grew past
  ~300 lines. Wire new routers into `main.rs`'s route composition the
  same way the existing ones are.
- **The sandbox map lock is a plain `std::sync::Mutex`, held across
  blocking calls inside `spawn_blocking`.** This is fine because those
  calls happen off the async runtime's threads, but don't assume you can
  `.await` while holding it — you can't, it's a sync mutex, not
  `tokio::sync::Mutex`, and that's deliberate (the lock only ever
  protects synchronous, fast-ish operations).
- Every route that boots or modifies a VM does real, possibly slow I/O
  (rootfs copy, network lease, Firecracker API calls) — that's why it
  runs inside `tokio::task::spawn_blocking`, not directly in an async
  handler. Follow that pattern for new VM-touching routes; don't block
  the async runtime's worker threads directly.
- **`/sandboxes/:id/preview/:port` is deliberately not behind the same
  bearer-token middleware as the rest of `/sandboxes*`.** It has its own
  (`auth::require_preview_token`) that accepts the token via a `?token=`
  query parameter as well as the `Authorization` header, because the
  thing hitting this URL is normally a browser tab or an `<iframe>`
  embedding a sandbox's dev server — neither can set a custom header on a
  plain navigation. This is still gated behind `SANDKILN_AUTH_TOKEN` when
  one is configured (no-op when it isn't, same as the rest of the API);
  the tradeoff accepted here is that a preview link, once handed out, is a
  bearer credential in URL form (referrer leakage, shell history, browser
  history) — reasonable for a short-lived dev-preview link, not something
  to reuse as a general auth pattern elsewhere in this API.
- **WebSocket proxying (for a dev server's HMR/live-reload) is explicitly
  out of scope for the initial `/preview` implementation.** The route
  proxies plain request/response HTTP; an `Upgrade: websocket` request
  currently just gets `Connection`/`Upgrade` stripped as hop-by-hop
  headers like any other, which will not upgrade correctly. Real support
  needs the daemon to detect the upgrade request, hijack both the
  client-facing and guest-facing connections, and pump bytes between them
  — a distinct enough problem (and untested without a live dev server
  actually using HMR) that it's a deliberate follow-up, not folded into
  this change.

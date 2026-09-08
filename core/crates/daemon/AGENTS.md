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
  `POST /sandboxes` request body can override. `archive_timeout`/
  `archive_dir` (`SANDKILN_ARCHIVE_TIMEOUT_SECS`/`SANDKILN_ARCHIVE_DIR`)
  configure `idle_reaper`'s archive pass — see that module's own doc
  comment and `archive_timeout`'s field doc comment for the real
  Firecracker constraint (the rootfs backing file can never move) that
  shapes what archiving actually does.
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
  *is* the daemon's entire notion of *live* sandbox state — it doesn't
  survive a restart, and per `sandkiln-store`'s own module doc comment
  there's no way it realistically ever could (a live `Sandbox` owns a
  real OS process with no re-adoption mechanism). `AppState::history`
  (a `sandkiln_store::HistoryStore`) is the separate, durable answer to
  a related but different question — not "is this sandbox still
  running" but "did this sandbox exist, and how did it end" — see
  `ROADMAP.md`'s "Tags and sandbox metadata" section. Also owns naming:
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
  Also owns `pools` (`Mutex<HashMap<String, crate::pool::Pool>>`) —
  configured pre-warmed pools, in-memory only (unlike `snapshots`, not
  reconciled from disk at startup — see `crate::pool`'s module doc
  comment for why).
- `sandbox.rs` — the `Sandbox` struct the daemon tracks per running VM
  (id, `Vm` handle, network `Lease`, rootfs path, tags, created-at,
  `last_activity`, `image_id` — the registered image this sandbox's
  rootfs was cloned from, if any, `None` meaning the daemon-wide
  `SANDKILN_BASE_ROOTFS` default — `jail_id`, the leased uid/gid if this
  sandbox booted jailed, released back to `state.jailer_ids` on stop —
  `name`, the caller-given identity carried across the sandbox<->snapshot
  boundary — and `pty_session_count`, an `Arc<AtomicU32>` so a
  `PtySessionGuard` (see `routes_pty.rs`) can outlive the `state.sandboxes`
  lock it was incremented under and still decrement the right counter on
  drop).
- `routes_sandbox.rs` — sandbox lifecycle handlers: create/list/stop/
  history (`GET /sandboxes/history`, reading `AppState::history` — the
  only read path for it; every write happens as a side effect of
  create/destroy/snapshot, not a caller action of its own).
  `create_sandbox_core` calls `state.history.record_created` right
  after a boot actually succeeds, and `destroy_sandbox_by_id` calls
  `state.history.record_ended` (`routes_snapshot::snapshot_and_stop`
  calls the equivalent for the snapshotted case) — both best-effort
  (a warning log, not a failed request, if the history write itself
  fails; the sandbox operation already succeeded by that point).
  `stop_sandbox_by_id()` is the shared stop entry point used by both the
  `DELETE` route and `idle_reaper`; it defaults to preserving state
  (snapshot-then-stop, via `routes_snapshot::snapshot_and_stop`) rather
  than destroying it, with `destroy_sandbox_by_id()` — the original
  teardown (VM stop, network release, rootfs cleanup) — reached via the
  `?keep=false` opt-out or as the correct silent fallback for a forked
  sandbox (nothing new to preserve) or a jailed one (can't be
  snapshotted, surfaces as an error instead of silently discarding
  state). Both `destroy_sandbox_by_id` and `routes_snapshot::snapshot_and_stop`
  also release a stopped sandbox's `source_pool_id` slot back to its pool
  (`Pool::record_release`) right after removing it from `state.sandboxes`
  — whichever way it stops, warm or claimed slots are the same
  `max_count` currency (see `crate::pool`). `create_sandbox_core()` is the actual boot logic, shared with
  `routes_sandbox_name::get_or_create_sandbox`'s create-fresh path — the
  `create_sandbox` handler itself adds the name-uniqueness check under
  `AppState::lock_name` and resolves which rootfs to clone from
  (`state.config.base_rootfs_path` by default, or a registered image's
  path when the request gives an `image_id`, via `AppState::images`,
  reserving/releasing a pending-boot claim on that image id around the
  whole boot with `PendingImageBootGuard` so a concurrent image deletion
  can't race an in-flight clone) before calling it.
  `create_sandbox_core` also tries a pre-warmed pool claim first (see
  `crate::pool`) whenever the request has no `drives`/`rate_limit` and a
  configured pool's key matches — `resolve_pool_claim` decides between
  `PoolClaim::Warm`/`ColdSlot`/`NoPool`, queueing (up to
  `POOL_QUEUE_TIMEOUT`, a `503` past that) on a `max_count`-bounded
  pool's own `Notify` if neither a warm snapshot nor headroom is
  available. A `Warm` claim's `claim_from_pool` resumes the snapshot,
  runs a real post-resume health check (`exec true`) before trusting it,
  and on failure releases its reserved slot (`PoolClaimGuard`'s `Drop`)
  and **retries** `resolve_pool_claim` (bounded, `MAX_POOL_CLAIM_ATTEMPTS`)
  before ever falling through to a plain, unattributed cold create — see
  `crate::pool`'s own module doc comment for the real Firecracker/KVM
  finding that made the health check load-bearing (not defensive
  theater) and the real bug the retry loop itself fixes (a failed
  claim's fallback silently not counting against `max_count`, found by
  live-testing this exact feature, not caught on paper). `ColdSlot`
  threads its reserved `pool_id` through `create_sandbox_cold` (the
  factored-out boot mechanics, shared by both the `ColdSlot` and
  unattributed paths) into `Sandbox::source_pool_id`, committing the
  `PoolClaimGuard` on success.
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
- `routes_exec.rs` — exec/read-file/write-file handlers. `pub(crate) async fn
  call_agent()` is the shared helper every route in both this file and
  `routes_fs.rs` uses — extend it, don't duplicate its pattern. It's
  also what bumps a sandbox's `last_activity`.
- `routes_fs.rs` — filesystem metadata/structure handlers: chmod, chown,
  mkdir, rename, copy, symlink, readlink, truncate, directory listing.
  Split out of `routes_exec.rs` (2026-09-08) once adding all of these
  there would have pushed it well past this crate's usual size range —
  data transfer (`routes_exec`) vs. filesystem structure (`routes_fs`)
  is the seam. No path validation on any handler here, same as
  `routes_exec::read_file`/`write_file` already have none — see this
  file's own module doc comment for why that's a deliberate consistency
  choice, not an oversight.
- `pool.rs` — `Pool`/`PoolConfig`/`PoolKey`: the pure state a configured
  pre-warmed pool tracks (its resolved image/resource key, a FIFO queue
  of warm snapshot ids, and `claimed` — how many live instances of this
  pool's profile currently exist) and the pure matching logic
  (`PoolConfig::key`) a `POST /sandboxes` request is checked against.
  `PoolConfig.max_count` bounds `claimed` (plus what's warm);
  `has_room_for_new_claim`/`record_claim`/`record_release` are the
  three operations that keep it honest, and `effective_warm_target`
  makes replenishment itself respect the same ceiling (never over-warms
  past the remaining headroom). `Pool::notify` (a `tokio::sync::Notify`,
  woken by `push_warm` and `record_release`) is what
  `routes_sandbox::resolve_pool_claim` waits on when a `max_count`-bounded
  pool is at capacity — condvar-style: every waiter re-checks the real
  condition on wake rather than trusting the wakeup itself. No
  networking, no `AppState` beyond that `Notify` — see this file's own
  module doc comment for the feature's full shape.
- `pool_replenisher.rs` — the background task (spawned unconditionally
  from `main.rs`, unlike `idle_reaper` below, since an idle tick with no
  pools configured is cheap) that keeps every pool topped up: boots a
  warm instance via `routes_sandbox::create_sandbox_core` (tagged
  `sandkiln.pool` for identifiability, nothing more), immediately
  `snapshot_and_stop`s it, and pushes the resulting snapshot id onto that
  pool's warm queue. One slot per pool per 2-second tick, not all at
  once, so a large `warm_count` fills in gradually rather than spiking
  boot load.
- `routes_pool.rs` — `POST/GET /pools`, `DELETE /pools/:id`: pool
  *configuration* only — replenishment lives in `pool_replenisher`,
  claiming lives in `routes_sandbox::create_sandbox_core`. `POST /pools`
  accepts `max_count` (rejects an explicit `0` — omit it for unbounded
  instead); `GET /pools` reports it alongside the live `claimed` count.
  `DELETE` destroys whatever the pool still has warm (via
  `routes_snapshot::delete_snapshot_by_id`) and calls
  `pool.notify.notify_waiters()` first, so anything queued on a
  `max_count`-bounded pool that just got deleted fails clearly instead of
  waiting out its own timeout for a pool that no longer exists.
- `idle_reaper.rs` — background task, spawned unconditionally from
  `main.rs` (a tick with nothing configured is a cheap no-op scan, same
  reasoning `pool_replenisher` already uses). Reclaims idle sandboxes two
  ways: auto-suspend (pause + snapshot, via
  `routes_snapshot::snapshot_and_stop`) past
  `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`, and destroy (via
  `routes_sandbox::stop_sandbox_by_id`, same preserve-by-default behavior
  as an explicit stop — see above — with a fallback to a real destroy only
  when preservation is structurally impossible, so an unpreservable idle
  sandbox doesn't leak forever) past `SANDKILN_IDLE_TIMEOUT_SECS`. Each
  tick runs auto-suspend first, then destroy against whatever's still
  running — see `config::Config::auto_suspend_timeout`'s doc comment for
  why `auto_suspend_timeout` is required to be strictly shorter than
  `idle_timeout` when both are set (destroy is a backstop for a
  persistently-failing auto-suspend, not a competing timer). Also runs a
  third, independent pass — `archive_idle_snapshots`, past
  `SANDKILN_ARCHIVE_TIMEOUT_SECS` — that moves a *held snapshot's* (any
  origin, not just auto-suspended ones) `state.snap`/`mem.bin` onto
  `Config::archive_dir` via `routes_snapshot::archive_snapshot_by_id`;
  see that function's own doc comment for why `rootfs_path` is
  deliberately never touched.
- `snapshot.rs` — the `Snapshot` type (`state.snapshots`'s value type)
  plus everything that makes it durable across a daemon restart: on-disk
  metadata (`meta.json`, alongside `state.snap`/`mem.bin` under
  `snapshot_dir(id)`) written atomically via write-then-rename, and
  `reconcile()`, which scans both `snapshots_root()` (hot) and
  `Config::archive_dir` (archived — always scanned, regardless of whether
  `SANDKILN_ARCHIVE_TIMEOUT_SECS` is currently set, so turning archiving
  off doesn't orphan snapshots already archived under it) and rebuilds
  `AppState::snapshots` from what's actually on disk across both — the
  same "filesystem is the source of truth" pattern `sandkiln_vmm::drive`'s
  `DriveStore::list()` uses for drives. A snapshot directory missing any
  of its three files is treated as a crash-mid-write (or crash mid-
  archive) and skipped with a warning rather than guessed at.
  `reconcile()` also calls `NetworkManager::reserve()` for each
  reconciled snapshot's held tap device/host octet so a live `lease()`
  call afterward can't hand the same tap to a second sandbox — see
  `main.rs`, which runs this before the HTTP listener starts accepting
  connections. `move_snapshot_files`/`move_file` are the archiving
  primitives (rename, falling back to copy-then-remove-original across
  filesystems) — **`move_snapshot_files` never touches `rootfs_path`**,
  see its own doc comment for the real Firecracker resume failure that
  proved moving it breaks every future resume/fork (the backing file's
  absolute host path is baked into `state.snap` itself, with no override
  at `/snapshot/load` time).
- `routes_drives.rs` / `routes_snapshot.rs` — drives and snapshot/resume
  handlers, each in their own file for the same reason as above. The
  actual pause/snapshot/stop mechanics live in `snapshot_and_stop()`, the
  actual resume mechanics in `resume_snapshot_by_id()`, the actual delete
  mechanics in `delete_snapshot_by_id()`, and the actual archive mechanics
  in `archive_snapshot_by_id()` — all four `pub(crate)`, all reused
  elsewhere in this crate (`snapshot_and_stop`/`resume_snapshot_by_id` by
  `routes_sandbox`'s persistent-by-default
  stop, `routes_sandbox_name`'s get-or-create, `idle_reaper`'s
  auto-suspend, and now `pool`/`pool_replenisher`/`routes_sandbox`'s
  claim path; `delete_snapshot_by_id` by `routes_pool::delete_pool`'s
  warm-snapshot cleanup) so there's exactly one place that knows what
  "snapshot this sandbox" / "resume this snapshot" / "delete this
  snapshot" / "archive this snapshot" means. `archive_snapshot_by_id` is
  only ever called by `idle_reaper` today — no `POST /snapshots/:id/archive`
  route exists yet to trigger it on demand, a deliberately deferred
  follow-up. `check_snapshottable`/
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
- `routes_pty.rs` — `GET /sandboxes/:id/pty[?cols=&rows=]`: upgrades to a
  WebSocket and proxies raw bytes to a shell running inside the sandbox,
  via `sandkiln_vmm::vm::Vm::open_pty` — a fundamentally different shape
  from every other route in this crate (all one-request-one-response;
  this is a live, long-lived, bidirectional session). Its own router in
  `main.rs`, guarded by `auth::require_preview_token` for exactly the
  same reason as `routes_preview.rs` above: neither a browser's nor
  Node.js's native `WebSocket` constructor can set custom headers, so
  header-only `require_bearer_token` auth can't work here. Enforces
  `MAX_PTY_SESSIONS_PER_SANDBOX` (64) via `Sandbox::pty_session_count`
  and a `PtySessionGuard` whose `Drop` decrements it on every exit path
  (clean close, error, or the task simply being dropped). See
  `sandkiln-guest-agent`'s `pty.rs` for the other end of the connection
  and the real hangup-handling bug found and fixed there — this route's
  own `proxy_pty` just needs both `tokio::select!` arms to end the
  session as soon as either side does, which was already correct; the
  bug was entirely guest-side.
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

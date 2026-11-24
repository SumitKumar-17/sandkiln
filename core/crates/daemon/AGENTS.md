# AGENTS.md — sandkiln-daemon

Read root `AGENTS.md` first. This file covers `sandkilnd`, the HTTP API.

## What this crate is

An axum + tokio HTTP server wrapping `sandkiln-vmm`'s VM lifecycle into a
REST-ish API — what SDKs/CLI actually talk to. Keep business logic (VM
lifecycle, networking) in `sandkiln-vmm`; this crate parses requests,
calls `vmm`, shapes responses.

## Layout

```
src/
  main.rs            bootstrap only: CLI, config load, storage init, wiring, serve, shutdown
  config/            settings.rs (registry) source.rs (env>file>default) cli.rs mod.rs (Config)
  http/              mod.rs build_router; auth.rs error.rs request_id.rs
    routes/          one file per API resource (sandbox, exec, fs, logs, pty, preview, tunnel,
                     mounts, drives, images, pool, snapshot, snapshot_history, metrics, ...)
  lifecycle/         create.rs create_options.rs stop.rs idle_reaper.rs pool.rs pool_claim.rs
                     pool_replenisher.rs shutdown.rs
  snapshots/         held.rs + held/reconcile.rs (live resumable snapshots) retired.rs (IP-free
                     checkpoints) layout.rs (shared on-disk layout) ops/ (take resume fork delete archive)
  state.rs + state/  AppState; holders.rs (who holds a drive/image/guest IP) naming.rs (names + name locks)
  sandbox.rs storage.rs metrics.rs log_session.rs tracing_util.rs
```

Dependency direction: `http` and `lifecycle` call into `state`/`snapshots`/`sandkiln-vmm`;
`snapshots/` and `state` hold no routing or request parsing (the operations return
`AppError` so a route can pass it straight through). Route files keep extraction,
validation and response shaping; the lifecycle logic lives in `lifecycle/` and
`snapshots/ops/`. A new endpoint is a new file in
`http/routes/` plus one line in `http/mod.rs`'s `build_router`.

## Files

- **`main.rs`** — not `#[tokio::main]` (see root `AGENTS.md` §12): raises
  `CAP_NET_ADMIN` before the Tokio runtime starts, then builds the
  router. Auth middleware wired onto `/sandboxes*` only; `/healthz` stays
  open.
- **`config/`** — every setting lives in one registry (`settings.rs`,
  `SETTINGS`): key, default, group, doc, secret flag. `source.rs`'s `Resolver`
  resolves each from the environment (`SANDKILN_<KEY>`), then `sandkiln.toml`,
  then the default, and *accumulates* errors into one `ConfigError` instead of
  panicking. `mod.rs` holds `Config` (the resolved, typed struct), `Config::load`
  / `Config::resolve` (the pure, unit-tested one), the cross-field checks
  (`vcpu <= max`, suspend before destroy), `render_effective` for
  `--print-config`, and `JailerHostConfig`/`LogFormat`. `cli.rs` parses
  `--config/--check-config/--print-config/--config-reference`. **Adding a
  setting = one `SETTINGS` entry, one `Config` field, one `resolve` line, one
  `display_value` arm, and a line in `sandkiln.example.toml`**; tests fail if any
  is missing.
- **`metrics.rs`** — hand-rolled Prometheus text format (no `prometheus`
  crate — four metrics behind atomics/a histogram isn't enough surface to
  justify the dependency). `sandboxes_created_total`, `boot_duration_ms`,
  `exec_latency_ms`, and `create_phase_duration_ms{phase=...}` (rootfs
  clone / network lease / setup / egress_apply / total — `setup` is the
  concurrent *join* of clone+lease, only the join is on the critical
  path; `egress_apply` only records when a create actually requests a
  policy). `boot_duration_ms` stays separate since it predates this
  family and is what external scrapers already expect. Phase totals an
  operator might alert on go here; per-PUT Firecracker detail stays a
  debug `tracing` event (also because `sandkiln-vmm` has no `Metrics`
  access).
- **`http/auth.rs`** — bearer-token middleware (constant-time comparison),
  no-op if `auth_token` is unset; `require_preview_token` also accepts `?token=`
  for browser-opened preview/WebSocket URLs.
- **`storage.rs`** — the daemon's on-disk layout (`state_dir`, `rootfs_dir`),
  process-global because snapshot path helpers have no `Config` in hand; plus
  `OwnedFile`, the scope guard that deletes a rootfs clone on any early return
  until ownership is handed to a live `Sandbox`.
- **`lifecycle/shutdown.rs`** — on SIGTERM/SIGINT: stop accepting, grace period
  for in-flight requests, then snapshot every live sandbox (destroy the ones that
  cannot be snapshotted). Verified by `scripts/restart-test.sh`.
- **`state.rs` / `state/`** — `state.rs` is `AppState` itself plus
  `AttachedDrive`/`Mount`; `state/holders.rs` and `state/naming.rs` extend it
  (re-exported from `state`). `AppState`: config, `NetworkManager`, optional
  `JailerIdPool`, in-memory `Mutex<HashMap<String, Sandbox>>` (the
  daemon's entire live-state notion — doesn't survive a restart, and per
  `sandkiln-store`'s own doc comment can't realistically be re-adopted).
  `AppState::history` is the separate durable "did this exist and how
  did it end" answer. Also owns: naming (`state/naming.rs`:
  `name_holder`/`resolve_name`, live wins over snapshot; `lock_name`
  serializes concurrent claims of one name); `drives`/`images` plus
  `drive_holders()`/`image_holder()` (`state/holders.rs`)
  ("who holds this" across live sandboxes + held snapshots, the
  mechanism that prevents double-attaching a drive); `pools` (in-memory
  only, not reconciled from disk — see `lifecycle/pool.rs`); `retired_snapshots`
  (time-travel restore) plus `guest_ip_holder()`, which names who holds a
  guest IP (live sandbox or held snapshot) for a `409` message. The
  exclusion itself is `NetworkManager::try_reserve_ip`, atomic with the
  free pool, so two sibling checkpoints cannot both restore. A held
  snapshot holds an IP and **no tap**; only a running VM holds a tap.
  `image_holder`/`guest_ip_holder` share a
  `first_match()` walk; `drive_holders()` stays separate since it
  returns *every* matching holder (multiple read-only holders are
  legitimate), not just the first. Also owns `tunnels`
  (`Mutex<HashMap<String, mpsc::Receiver<TunnelConnection>>>`) — the
  handoff between `POST /sandboxes/:id/tunnel` (registers, stores the
  receiver) and the matching `GET .../tunnel/:tunnel_id/ws` (takes it,
  `409` if already taken or never created) — see `http/routes/tunnel.rs`.
- **`sandbox.rs`** — `Sandbox`: id, `Vm` handle, `Lease` (IP + tap) or, for a fork,
  `fork_network` (just its tap; the snapshot keeps the IP), rootfs path,
  tags, timestamps, `image_id` (`None` = daemon default rootfs),
  `jail_id`, `name`, `pty_session_count` (`Arc<AtomicU32>`, survives past
  the lock it was incremented under), `egress` (`None` for a fork — its chain
  is keyed by the fork's tap and removed with it via `fork_network`),
  `parent_snapshot_id` (lineage; a **separate** field from
  `source_snapshot_id` — conflating them was a real bug: `source_snapshot_id.is_some()`
  is what refuses re-snapshotting a fork, and must stay `None` on resume
  so a resumed sandbox stays snapshottable, while lineage needs `Some`
  on both), and `env` (unlike `egress`, identical on resume *and* fork —
  plain data, no external resource to protect).
- **`http/routes/sandbox.rs`** — create/list/stop/history: the request/response
  DTOs (`CreateSandboxRequest` and friends), listing, history, query parsing
  and thin handlers over `lifecycle/create.rs` and `lifecycle/stop.rs`.
- **`lifecycle/create.rs`** — the create path. `create_sandbox_core()` is the
  shared boot logic (also used by `get_or_create_sandbox` and the pool
  replenisher): resolves drive conflicts and resource overrides, tries a
  pre-warmed pool claim first when the request has no `drives`/`rate_limit`,
  and falls back to `create_sandbox_cold`, which times the rootfs clone and
  network lease each on their own thread (`timed` + `thread::scope`) and
  records sub-phases via `metrics::CreatePhase` — filter with
  `RUST_LOG=sandkilnd=debug` (the **binary** name; `sandkiln_daemon` matches
  nothing). `PendingImageBootGuard` reserves a pending-image-boot claim around
  the clone so a concurrent `DELETE /images/:id` can't race it. `clone_rootfs`
  is the one way any mutating sandbox gets its own disk. Egress doesn't
  disqualify a pool claim (it's host-side iptables applied after boot, not
  baked into Firecracker state) — fatal-on-failure for a fresh create/claim,
  loud-warning-only for resume/fork (destroying a one-way-operation success
  over an iptables hiccup would be worse).
- **`lifecycle/create_options.rs`** — pure validation of the optional create
  knobs: `resolve_resource_override`, `resolve_rate_limit`,
  `resolve_egress_policy` (validates every CIDR up front, `400` naming the bad
  one) and `first_duplicate`.
- **`lifecycle/stop.rs`** — `stop_sandbox_by_id()` is the shared stop path
  (used by `DELETE`, `idle_reaper` and `shutdown`): defaults to
  snapshot-then-stop, `destroy_sandbox_by_id()` reached via `?keep=false` or as
  the correct fallback for a fork (nothing to preserve) or a jailed sandbox
  (can't snapshot). Both release a stopped sandbox's `source_pool_id` slot back
  to its pool. `StopOutcome`/`StopError` carry no HTTP type; the route maps
  `StopError::CannotPreserve` through its own `cannot_preserve_error`.
- **`http/routes/sandbox_name.rs`** — `GET /sandboxes/by-name/:name` (live
  only; a name held by a snapshot is `409`, not a silent resume) and
  `POST /sandboxes/get-or-create` (race-safe under `lock_name`).
- **`http/routes/images.rs`** — register/list/delete a managed rootfs.
  `guest_agent_verified: false` always — the daemon can't loop-mount to
  check; `scripts/preflight-check.sh --root-checks` is the out-of-band
  way. `DELETE` refuses while any live/in-flight/held reference exists.
- **`http/routes/exec.rs`** — exec/read-file/write-file;
  `call_agent[_with_timeout]()` is the shared helper (also `fs.rs`): extend
  it. `exec`'s `timeout_secs` (`resolve_exec_timeout`, else 400) is enforced
  by the guest: `TimedOut` is a 504 with `timed_out: true`. `resolve_env()`
  merges create-time + per-call `env`; `logs.rs` inlines it (holds the lock).
- **`http/routes/fs.rs`** — chmod/chown/mkdir/rename/copy/symlink/readlink/
  truncate/list-dir. No path validation here, matching
  `read_file`/`write_file` — deliberate consistency, not an oversight.
- **`http/routes/mounts.rs`** — `rclone mount` of an S3-compatible bucket,
  built entirely on `call_agent`/`Mkdir`/`WriteFile`/`Chmod`/`Exec` — no
  new wire protocol. Credentials go in as a `0600` config file, never a
  CLI arg. No holder-tracking (concurrent mounts of one bucket aren't a
  corruption risk) and no re-application on resume/fork (a mount is a
  live guest FUSE process, captured by Firecracker's own snapshot).
- **`lifecycle/pool.rs`** — pure pool state/matching logic; see
  `docs-site/internals/pre-warmed-pools.mdx` for the full design.
  `Pool::notify` wakes `pool_claim::resolve_pool_claim` on a
  `max_count`-bounded pool — condvar-style, every waiter re-checks on
  wake.
- **`lifecycle/pool_replenisher.rs`** — unconditional background task, tops up
  every pool one warm slot per 2s tick (gradual, not a load spike).
- **`lifecycle/pool_claim.rs`** — `resolve_pool_claim`/`PoolClaim`
  (`Warm`/`ColdSlot`/`NoPool`), `PoolClaimGuard` (RAII commit-or-release),
  `claim_from_pool` (resume + health check + bounded `Vm::update_metadata`
  retry for the MMDS settling-window race). Split out of
  `lifecycle/create.rs` once claiming pushed it past a defensible size.
- **`http/routes/pool.rs`** — pool *configuration* only (`max_count`, rejects
  explicit `0` — omit for unbounded). `DELETE` wakes queued waiters first
  so they fail clearly instead of timing out on a pool that's gone.
- **`lifecycle/idle_reaper.rs`** — unconditional background task; see
  `docs-site/internals/pre-warmed-pools.mdx`. Runs auto-suspend, then
  destroy, then an independent archive pass each tick.
- **`snapshots/held.rs` / `snapshots/held/reconcile.rs`** — `Snapshot` +
  durability: atomic `meta.json` write-then-rename, and `reconcile()` rebuilds
  `AppState::snapshots` from disk (hot dir + archive dir, filesystem is the
  source of truth, same pattern as `DriveStore::list()`) — a snapshot missing
  any of its three files is treated as a crash-mid-write and skipped with a
  warning. `Snapshot.egress`/`parent_snapshot_id` are `#[serde(default)]` for
  forward compat. `move_snapshot_files` **never touches `rootfs_path`** —
  its absolute host path is baked into `state.snap` itself, moving it
  breaks every future resume/fork (proved live).
- **`snapshots/layout.rs`** — where a snapshot directory lives (hot root,
  archive root), the three fixed filenames, `write_atomically` and `move_file`;
  shared by `held` and `retired`. `snapshots/test_support.rs` is the
  `cfg(test)` fixture (`TempDir`, `test_network`).
- **`snapshots/retired.rs`** — `RetiredSnapshot` (time-travel restore):
  resuming retires a snapshot here instead of deleting it, restorable
  repeatedly. Holds a bare `GuestAddr` + host octet, not an `IpLease` — nothing
  is reserved from the free pool just by sitting in history, only at
  actual restore time via `try_reserve_ip`. No tap is recorded.
- **`http/routes/snapshot_history.rs`** — history list/restore/delete.
  `restore_snapshot_history_by_id` refuses (`409`) if the checkpoint's
  guest IP is already live/held (or claimed by a concurrent restore), else clones rootfs
  fresh, claims the IP and leases a free tap. Resulting sandbox:
  `source_snapshot_id: None`, `parent_snapshot_id: Some(checkpoint)` —
  restoring doesn't consume it.
- **`snapshots/ops/`** — the real mechanics, one file per operation, all
  `pub(crate)` and reused everywhere else in the crate so there's exactly one
  definition of each: `take.rs` (`snapshot_and_stop`, `check_snapshottable`,
  `SnapshotStopError`), `resume.rs` (`resume_snapshot_by_id`, `resume_vm` — leases the tap and
  returns it with the `Vm`, `retire_snapshot_files`), `fork.rs` (`fork_snapshot_by_id`), `delete.rs`
  (`delete_snapshot_by_id`), `archive.rs` (`archive_snapshot_by_id`,
  `ArchiveError`); `mod.rs` holds `check_no_live_fork`, the guard behind
  resume/fork/delete. `check_snapshottable` refuses a jailed sandbox (can never
  be resumed correctly) or a fork (doesn't own its IP outright).
  `resume_snapshot_by_id`'s `retain_history: bool` defaults to retiring
  (not deleting) the checkpoint — falls back to delete-and-reuse as a
  loud warning, not a fatal error, if retiring fails after the VM already
  resumed. `fork_snapshot_by_id()` clones the rootfs BEFORE resuming and resumes
  onto the clone via `ResumeConfig::rootfs_override`, so the snapshot's own disk
  is never written (a resumed VM otherwise writes the host path baked into the
  snapshot: a real corruption bug, regression test topic 26). Retain-history
  resume and history restore do the same; `retained_checkpoints_max` caps
  retired checkpoints.
- **`http/routes/snapshot.rs`** — the snapshot resource's routing, DTOs
  (`SnapshotSummary`, responses), `?retain_history=` / list-filter query
  parsing and thin handlers over `snapshots/ops/`. The design notes for
  snapshot/resume/fork sit in this file's module doc.
- **`http/routes/drives.rs`** — drive create/list/delete; `DriveAttachment`.
- **`http/routes/metrics.rs`** — unauthenticated like `/healthz` (operational
  data, not sandbox data).
- **`http/routes/preview.rs`** — reverse-proxies to
  `http://<guest ip>:<port>/<path>` on the bridge network via a pooled
  `hyper_util` client. Own router, `require_preview_token` (a browser
  can't set `Authorization` on a plain navigation — token stripped,
  along with any real `Authorization` header, before forwarding to the
  untrusted guest). `BadGateway`/`GatewayTimeout` on
  unreachable/timeout.
- **`http/routes/pty.rs`** — WebSocket → `Vm::open_pty` raw byte proxy, a
  fundamentally different (long-lived, bidirectional) shape from every
  other route. `require_preview_token` for the same header-limitation
  reason. `MAX_PTY_SESSIONS_PER_SANDBOX` (64) via a `PtySessionGuard`
  whose `Drop` always decrements.
- **`log_session.rs` / `http/routes/logs.rs`** — `POST .../exec-stream` starts
  a detached command and returns immediately; a `spawn_blocking` pump
  reads framed events into a `LogSession` (1MiB ring buffer +
  `broadcast` channel) independent of whether anyone's attached.
  `GET .../logs` replays then live-tails, callable any number of times.
  Not carried across resume/fork/restart.
- **`http/routes/tunnel.rs`** — local tunnel: `POST /sandboxes/:id/tunnel`
  (registers with `sandkiln_vmm::tunnel`, sends `StartTunnel` over vsock,
  stores the receiver in `AppState::tunnels`) / `DELETE .../tunnel/:id`
  (bearer-gated, normal router) and `GET .../tunnel/:id/ws` (its own
  router, `require_preview_token` — even though the caller is always
  SDK/CLI code, never a browser, no WebSocket constructor in any runtime
  can set a custom header, so the `?token=` fallback applies here too).
  `proxy_tunnel` multiplexes every guest-accepted connection over that
  one WebSocket with a small binary frame (`op` byte + `conn_id` +
  payload — `open`/`data`/`close`), using a `tokio::sync::Mutex`-guarded
  `writers` map since writing to a connection's socket has to `.await`
  while holding the lock. Bridges `sandkiln_vmm::tunnel`'s blocking
  `std::sync::mpsc::Receiver` into async via a plain `spawn_blocking`
  forwarding into a `tokio::sync::mpsc` channel.
- **`error.rs`** — `AppError`, the one error type every handler returns.
- **`request_id.rs`** — `correlate`, the outermost `main.rs` layer:
  resolves a request id (caller's `X-Request-Id` if well-formed, else a
  fresh UUID), echoes it on the response, wraps the request in a
  `tracing::Span` carrying it so every log line for that request —
  including ones from deep inside `sandkiln-vmm`'s `spawn_blocking`
  calls — carries the same id.
- **`tracing_util.rs`** — `spawn_blocking_in_current_span`: `tracing`'s
  "current span" is thread-local, so a bare `spawn_blocking` silently
  drops request correlation; this re-establishes both the span and the
  default dispatcher on the blocking-pool thread.

## Building and verifying

`cargo build -p sandkiln-daemon` catches compile errors only — every
route needs live verification against a real running daemon (the
DELETE-status-code bug compiled and clippy-passed cleanly but was still
wrong).

## Non-obvious things

- New route handlers touching the sandbox map or `vmm` get their own
  its own `http/routes/*.rs` file — a shared file is a guaranteed merge-conflict point for
  parallel agents.
- The sandbox map lock is `std::sync::Mutex`, held across blocking calls
  inside `spawn_blocking` — never `.await` while holding it.
- VM-touching routes do real, possibly slow I/O — run them in
  `spawn_blocking`, don't block the async runtime directly.
- `/preview` isn't behind the normal bearer middleware — it accepts
  `?token=` because a browser tab/`<iframe>` can't set a header. Still
  gated behind `SANDKILN_AUTH_TOKEN` when one's configured. Accepted
  tradeoff: a handed-out preview link is a bearer credential in URL form.
- WebSocket proxying (dev-server HMR) is out of scope for `/preview` —
  an `Upgrade` request just gets its headers stripped like any other
  hop-by-hop header, won't upgrade correctly. Real support needs the
  daemon to hijack both connections and pump bytes — a distinct,
  deliberate follow-up.

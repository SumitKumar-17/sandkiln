# AGENTS.md — sandkiln-daemon

Read root `AGENTS.md` first. This file covers `sandkilnd`, the HTTP API.

## What this crate is

An axum + tokio HTTP server wrapping `sandkiln-vmm`'s VM lifecycle into a
REST-ish API — what SDKs/CLI actually talk to. Keep business logic (VM
lifecycle, networking) in `sandkiln-vmm`; this crate parses requests,
calls `vmm`, shapes responses.

## Files

- **`main.rs`** — not `#[tokio::main]` (see root `AGENTS.md` §12): raises
  `CAP_NET_ADMIN` before the Tokio runtime starts, then builds the
  router. Auth middleware wired onto `/sandboxes*` only; `/healthz` stays
  open.
- **`config.rs`** — `Config::from_env()`, every `SANDKILN_*` env var in
  one place. `LogFormat` (`SANDKILN_LOG_FORMAT`), `JailerHostConfig`
  (`SANDKILN_JAILER_ENABLED` — daemon-operator switch, not a per-request
  override), `archive_timeout`/`archive_dir` for `idle_reaper`'s archive
  pass.
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
- **`auth.rs`** — bearer-token middleware, no-op if
  `SANDKILN_AUTH_TOKEN` is unset.
- **`state.rs`** — `AppState`: config, `NetworkManager`, optional
  `JailerIdPool`, in-memory `Mutex<HashMap<String, Sandbox>>` (the
  daemon's entire live-state notion — doesn't survive a restart, and per
  `sandkiln-store`'s own doc comment can't realistically be re-adopted).
  `AppState::history` is the separate durable "did this exist and how
  did it end" answer. Also owns: naming (`name_holder`/`resolve_name`,
  live wins over snapshot; `lock_name` serializes concurrent claims of
  one name); `drives`/`images` plus `drive_holder()`/`image_holder()`
  ("who holds this" across live sandboxes + held snapshots, the
  mechanism that prevents double-attaching a drive); `pools` (in-memory
  only, not reconciled from disk — see `pool.rs`); `retired_snapshots`
  (time-travel restore) plus `tap_device_holder()`, the same ownership
  check extended to cover a lease-less retired checkpoint and an
  in-flight restore. `image_holder`/`tap_device_holder` share a
  `first_match()` walk; `drive_holders()` stays separate since it
  returns *every* matching holder (multiple read-only holders are
  legitimate), not just the first.
- **`sandbox.rs`** — `Sandbox`: id, `Vm` handle, `Lease`, rootfs path,
  tags, timestamps, `image_id` (`None` = daemon default rootfs),
  `jail_id`, `name`, `pty_session_count` (`Arc<AtomicU32>`, survives past
  the lock it was incremented under), `egress` (`None` for a fork — tied
  to the lease, which the snapshot owns, not the forked record),
  `parent_snapshot_id` (lineage; a **separate** field from
  `source_snapshot_id` — conflating them was a real bug: `source_snapshot_id.is_some()`
  is what refuses re-snapshotting a fork, and must stay `None` on resume
  so a resumed sandbox stays snapshottable, while lineage needs `Some`
  on both), and `env` (unlike `egress`, identical on resume *and* fork —
  plain data, no external resource to protect).
- **`routes_sandbox.rs`** — create/list/stop/history.
  `create_sandbox_cold` times the rootfs clone and network lease each on
  their own thread (`timed` + `thread::scope`) and records sub-phases via
  `metrics::CreatePhase` — filter with `RUST_LOG=sandkilnd=debug` (the
  **binary** name; `sandkiln_daemon` matches nothing).
  `stop_sandbox_by_id()` is the shared stop path (used by `DELETE` and
  `idle_reaper`): defaults to snapshot-then-stop,
  `destroy_sandbox_by_id()` reached via `?keep=false` or as the correct
  fallback for a fork (nothing to preserve) or a jailed sandbox (can't
  snapshot). Both release a stopped sandbox's `source_pool_id` slot back
  to its pool. `create_sandbox_core()` is the shared boot logic (also
  used by `get_or_create_sandbox`): resolves name-uniqueness, which
  rootfs to clone, reserves a pending-image-boot claim around the clone
  so a concurrent `DELETE /images/:id` can't race it, and tries a
  pre-warmed pool claim first when the request has no `drives`/
  `rate_limit`. `resolve_egress_policy` validates every CIDR up front
  (`400` naming the bad one); unlike `drives`/`rate_limit`, egress
  doesn't disqualify a pool claim (it's host-side iptables applied after
  boot, not baked into Firecracker state) — fatal-on-failure for a fresh
  create/claim, loud-warning-only for resume/fork (destroying a
  one-way-operation success over an iptables hiccup would be worse).
- **`routes_sandbox_name.rs`** — `GET /sandboxes/by-name/:name` (live
  only; a name held by a snapshot is `409`, not a silent resume) and
  `POST /sandboxes/get-or-create` (race-safe under `lock_name`).
- **`routes_images.rs`** — register/list/delete a managed rootfs.
  `guest_agent_verified: false` always — the daemon can't loop-mount to
  check; `scripts/preflight-check.sh --root-checks` is the out-of-band
  way. `DELETE` refuses while any live/in-flight/held reference exists.
- **`routes_exec.rs`** — exec/read-file/write-file. `call_agent()` is the
  shared helper (also used by `routes_fs.rs`) — extend it, don't
  duplicate. `resolve_env()` merges create-time + per-call `env` (call
  wins); `routes_logs.rs` inlines the same merge rather than importing
  it since it already holds the sandbox-map lock at that point.
- **`routes_fs.rs`** — chmod/chown/mkdir/rename/copy/symlink/readlink/
  truncate/list-dir. No path validation here, matching
  `read_file`/`write_file` — deliberate consistency, not an oversight.
- **`routes_mounts.rs`** — `rclone mount` of an S3-compatible bucket,
  built entirely on `call_agent`/`Mkdir`/`WriteFile`/`Chmod`/`Exec` — no
  new wire protocol. Credentials go in as a `0600` config file, never a
  CLI arg. No holder-tracking (concurrent mounts of one bucket aren't a
  corruption risk) and no re-application on resume/fork (a mount is a
  live guest FUSE process, captured by Firecracker's own snapshot).
- **`pool.rs`** — pure pool state/matching logic; see
  `docs/architecture/02-vm-boot-and-latency.md` for the full design.
  `Pool::notify` wakes `pool_claim::resolve_pool_claim` on a
  `max_count`-bounded pool — condvar-style, every waiter re-checks on
  wake.
- **`pool_replenisher.rs`** — unconditional background task, tops up
  every pool one warm slot per 2s tick (gradual, not a load spike).
- **`pool_claim.rs`** — `resolve_pool_claim`/`PoolClaim`
  (`Warm`/`ColdSlot`/`NoPool`), `PoolClaimGuard` (RAII commit-or-release),
  `claim_from_pool` (resume + health check + bounded `Vm::update_metadata`
  retry for the MMDS settling-window race). Split out of
  `routes_sandbox.rs` once claiming pushed it past a defensible size.
- **`routes_pool.rs`** — pool *configuration* only (`max_count`, rejects
  explicit `0` — omit for unbounded). `DELETE` wakes queued waiters first
  so they fail clearly instead of timing out on a pool that's gone.
- **`idle_reaper.rs`** — unconditional background task; see
  `docs/architecture/02-vm-boot-and-latency.md`. Runs auto-suspend, then
  destroy, then an independent archive pass each tick.
- **`snapshot.rs`** — `Snapshot` + durability: atomic `meta.json`
  write-then-rename, `reconcile()` rebuilds `AppState::snapshots` from
  disk (hot dir + archive dir, filesystem is the source of truth, same
  pattern as `DriveStore::list()`) — a snapshot missing any of its three
  files is treated as a crash-mid-write and skipped with a warning.
  `Snapshot.egress`/`parent_snapshot_id` are `#[serde(default)]` for
  forward compat. `move_snapshot_files` **never touches `rootfs_path`** —
  its absolute host path is baked into `state.snap` itself, moving it
  breaks every future resume/fork (proved live).
- **`snapshot_history.rs`** — `RetiredSnapshot` (time-travel restore):
  resuming retires a snapshot here instead of deleting it, restorable
  repeatedly. Holds a bare `NetworkConfig`, not a live `Lease` — nothing
  is reserved from the free pool just by sitting in history, only at
  actual restore time, gated by `tap_device_holder`.
- **`routes_snapshot_history.rs`** — history list/restore/delete.
  `restore_snapshot_history_by_id` refuses (`409`) if the checkpoint's
  network identity is already live/held/mid-restore, else clones rootfs
  fresh and reserves a new lease. Resulting sandbox:
  `source_snapshot_id: None`, `parent_snapshot_id: Some(checkpoint)` —
  restoring doesn't consume it.
- **`routes_drives.rs` / `routes_snapshot.rs`** — the real mechanics
  (`snapshot_and_stop`, `resume_snapshot_by_id`, `delete_snapshot_by_id`,
  `archive_snapshot_by_id`, all `pub(crate)`) live here, reused
  everywhere else in the crate so there's exactly one definition of each
  operation. `check_snapshottable` refuses a jailed sandbox (can never be
  resumed correctly) or a fork (doesn't own its lease outright).
  `resume_snapshot_by_id`'s `retain_history: bool` defaults to retiring
  (not deleting) the checkpoint — falls back to delete-and-reuse as a
  loud warning, not a fatal error, if retiring fails after the VM already
  resumed. `fork_snapshot()` gives every fork its own private rootfs
  clone (fixes a real sequential-corruption bug, found live).
- **`routes_metrics.rs`** — unauthenticated like `/healthz` (operational
  data, not sandbox data).
- **`routes_preview.rs`** — reverse-proxies to
  `http://<guest ip>:<port>/<path>` on the bridge network via a pooled
  `hyper_util` client. Own router, `require_preview_token` (a browser
  can't set `Authorization` on a plain navigation — token stripped,
  along with any real `Authorization` header, before forwarding to the
  untrusted guest). `BadGateway`/`GatewayTimeout` on
  unreachable/timeout.
- **`routes_pty.rs`** — WebSocket → `Vm::open_pty` raw byte proxy, a
  fundamentally different (long-lived, bidirectional) shape from every
  other route. `require_preview_token` for the same header-limitation
  reason. `MAX_PTY_SESSIONS_PER_SANDBOX` (64) via a `PtySessionGuard`
  whose `Drop` always decrements.
- **`log_session.rs` / `routes_logs.rs`** — `POST .../exec-stream` starts
  a detached command and returns immediately; a `spawn_blocking` pump
  reads framed events into a `LogSession` (1MiB ring buffer +
  `broadcast` channel) independent of whether anyone's attached.
  `GET .../logs` replays then live-tails, callable any number of times.
  Not carried across resume/fork/restart.
- **`error.rs`** — `AppError`, the one error type every handler returns.

## Building and verifying

`cargo build -p sandkiln-daemon` catches compile errors only — every
route needs live verification against a real running daemon (the
DELETE-status-code bug compiled and clippy-passed cleanly but was still
wrong).

## Non-obvious things

- New route handlers touching the sandbox map or `vmm` get their own
  `routes_*.rs` — a shared file is a guaranteed merge-conflict point for
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

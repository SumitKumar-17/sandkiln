# AGENTS.md: sandkiln-daemon

Read root `AGENTS.md` first. This crate is `sandkilnd`, the axum + tokio HTTP
API that SDKs and the CLI talk to. Keep VM lifecycle and networking logic in
`sandkiln-vmm`; this crate parses requests, calls `vmm`, and shapes responses.

## Layout

```
src/
  main.rs            bootstrap only: CLI, config load, storage init, wiring, serve, shutdown
  config/            settings.rs (registry), source.rs (env > file > default), cli.rs, mod.rs (Config)
  http/              mod.rs (build_router), auth.rs, error.rs, request_id.rs
    routes/          one file per API resource: sandbox, sandbox_name, exec, fs, logs, pty, preview,
                     tunnel, mounts, drives, images, pool, snapshot, snapshot_history, metrics, status
  lifecycle/         idle_reaper.rs, pool.rs, pool_claim.rs, pool_replenisher.rs, shutdown.rs
  snapshots/         held.rs (live resumable snapshots), retired.rs (lease-free checkpoints)
  state.rs sandbox.rs storage.rs metrics.rs log_session.rs tracing_util.rs
```

Dependency direction: `http` and `lifecycle` call into `state`, `snapshots` and
`sandkiln-vmm`. Nothing under `snapshots/` or `state` knows about HTTP. A new
endpoint is a new file in `http/routes/` plus one line in `build_router`.

## Modules

- **`main.rs`**: plain `fn main()`, not `#[tokio::main]` (root `AGENTS.md` section 12). Raises
  `CAP_NET_ADMIN` before the runtime starts. Auth middleware covers `/sandboxes*`; `/healthz` stays open.
- **`config/`**: every setting is one `SETTINGS` registry entry (key, default, group, doc,
  secret flag). `Resolver` reads env, then `sandkiln.toml`, then the default, and accumulates
  all errors into one `ConfigError`. Adding a setting means one `SETTINGS` entry, one `Config`
  field, one `resolve` line, one `display_value` arm and a line in `sandkiln.example.toml`;
  tests fail if any is missing. `Config::resolve` is the pure, unit-tested path.
- **`state.rs`**: `AppState` holds config, `NetworkManager`, the optional `JailerIdPool`, the
  in-memory `Mutex<HashMap<String, Sandbox>>` (all live state; lost on restart), and
  `AppState::history`, the durable record of how sandboxes ended. It also owns naming
  (`name_holder`, `resolve_name`, `lock_name`), `drive_holder()`/`image_holder()`/`tap_device_holder()`
  ("who holds this" across live sandboxes and held snapshots, which prevents double-attaching),
  in-memory `pools`, `retired_snapshots`, and `tunnels` (the handoff between the tunnel POST and
  its WebSocket GET).
- **`sandbox.rs`**: the `Sandbox` record. `parent_snapshot_id` (lineage) is deliberately separate
  from `source_snapshot_id` (which refuses re-snapshotting a fork); conflating them was a real bug.
  `egress` is `None` on a fork (the lease belongs to the snapshot); `env` is identical on resume and fork.
- **`storage.rs`**: on-disk layout (`state_dir`, `rootfs_dir`), process-global; `OwnedFile` deletes
  a rootfs clone on early return until a live `Sandbox` owns it.
- **`metrics.rs`**: hand-rolled Prometheus text. `create_phase_duration_ms{phase=...}` records clone,
  lease, setup (the concurrent join), `egress_apply` and total. `boot_duration_ms` stays separate
  for existing scrapers. Per-PUT Firecracker detail stays a debug `tracing` event.
- **`http/auth.rs`**: constant-time bearer check, no-op without `auth_token`. `require_preview_token`
  also accepts `?token=` for browser and WebSocket URLs, since neither can set a header; it strips
  the token and any `Authorization` header before proxying to the guest.
- **`http/routes/sandbox.rs`**: create, list, stop, history. `create_sandbox_core` is the shared
  boot path (also used by `get-or-create`): name uniqueness, rootfs choice, a pending-image claim
  against concurrent `DELETE /images/:id`, and a pool claim when the request has no `drives` or
  `rate_limit`. `stop_sandbox_by_id` snapshots then stops by default; `destroy_sandbox_by_id` runs for
  `?keep=false`, forks and jailed sandboxes. Egress CIDRs are validated up front; a failed policy is
  fatal on create and a loud warning on resume/fork. Run with `RUST_LOG=sandkilnd=debug` (binary name).
- **`http/routes/exec.rs`, `fs.rs`, `logs.rs`**: `call_agent()` in `exec.rs` is the shared guest call;
  extend it. `resolve_env()` merges create-time and per-call env (call wins). `fs.rs` does no path
  validation, matching `read_file`/`write_file`. Exec-stream sessions pump into a `LogSession`
  (1MiB ring plus broadcast) that replays then live-tails; not carried across resume, fork or restart.
- **`http/routes/mounts.rs`**: `rclone mount` built from existing agent calls; credentials as a `0600`
  config file, never a CLI arg. Mounts are not re-applied on resume (the FUSE process is in the snapshot).
- **`http/routes/images.rs`**: `guest_agent_verified` is always `false`; the daemon cannot loop-mount.
  `DELETE` refuses while anything references the image.
- **`http/routes/snapshot.rs`, `drives.rs`**: `snapshot.rs` holds the one definition of each snapshot
  operation (`snapshot_and_stop`, `resume_snapshot_by_id`, `delete_snapshot_by_id`,
  `archive_snapshot_by_id`, all `pub(crate)`). `check_snapshottable` refuses jailed sandboxes and forks.
  Fork, retain-history resume and history restore clone the rootfs first and resume onto the clone via
  `ResumeConfig::rootfs_override`, so a snapshot's own disk is never written (regression: integration
  topic 26). `retained_checkpoints_max` caps retired checkpoints.
- **`snapshots/held.rs`**: `Snapshot` plus durability: atomic `meta.json` write-then-rename and
  `reconcile()`, which rebuilds state from disk (hot and archive dirs); a snapshot missing any of its
  three files is skipped as crash-mid-write. `move_snapshot_files` never moves `rootfs_path`, because
  Firecracker bakes its absolute path into `state.snap`.
- **`snapshots/retired.rs`, `http/routes/snapshot_history.rs`**: time-travel checkpoints. A retired
  checkpoint holds a bare `NetworkConfig`, not a `Lease`; restore returns `409` if that network identity
  is live, held or mid-restore, and does not consume the checkpoint.
- **`http/routes/preview.rs`**: reverse proxy to `http://<guest ip>:<port>/<path>` through a pooled
  `hyper_util` client; no WebSocket upgrade support (headers stripped as hop-by-hop).
- **`http/routes/pty.rs`**: WebSocket to `Vm::open_pty` byte proxy, capped at 64 sessions per sandbox
  by a `PtySessionGuard` whose `Drop` always decrements.
- **`http/routes/tunnel.rs`**: registers with `sandkiln_vmm::tunnel`, sends `StartTunnel`, and stores the
  receiver in `AppState::tunnels`; the WS route multiplexes guest connections with a small binary frame
  (`op`, `conn_id`, payload).
- **`http/routes/pool.rs`, `lifecycle/pool*.rs`**: `pool.rs` (lifecycle) is pure state and matching, with
  `Pool::notify` waking `resolve_pool_claim` on a `max_count` pool. `pool_replenisher` tops up one warm
  slot per pool every 2s. `pool_claim` does resume, health check and a bounded `Vm::update_metadata`
  retry. The route handles configuration only and rejects an explicit `max_count` of `0`. Design:
  `docs-site/internals/pre-warmed-pools.mdx`.
- **`lifecycle/idle_reaper.rs`**: each tick runs auto-suspend, then destroy, then an independent archive pass.
- **`lifecycle/shutdown.rs`**: on SIGTERM/SIGINT, stop accepting, grace period, snapshot every live
  sandbox, destroy those that cannot be snapshotted. `scripts/restart-test.sh` verifies it.
- **`http/error.rs`, `request_id.rs`, `tracing_util.rs`**: `AppError` is the one handler error type.
  `correlate` is the outermost layer: it echoes or generates `X-Request-Id` and wraps the request in a span.
  `spawn_blocking_in_current_span` carries span and dispatcher onto blocking threads; a bare
  `spawn_blocking` drops correlation.
- **`http/routes/metrics.rs`**: unauthenticated like `/healthz`.

## Invariants

- The sandbox map lock is a `std::sync::Mutex`. Never `.await` while holding it.
- VM-touching routes do slow blocking I/O: use `spawn_blocking`.
- New routes get their own file under `http/routes/`; a shared file is a merge-conflict point.
- A preview link is a bearer credential in URL form; accepted because a browser cannot set a header.

## Testing

`cargo test -p sandkiln-daemon` covers pure logic (config resolution, `idle_reaper::is_idle`,
`auth::token_matches`, pool matching). Compiling proves nothing for routes: add a case to
`scripts/integration-tests/` and run `scripts/integration-test.sh` against a real daemon.

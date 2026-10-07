# The daemon HTTP API, auth, and observability

`sandkilnd` (`core/crates/daemon`) is a single `axum` + `tokio` process exposing
the entire sandbox lifecycle over plain HTTP/JSON. It runs **unprivileged** — see
[07-security-and-privilege.md](07-security-and-privilege.md) for why that's a hard
requirement, not a nicety, given what it's booting.

## What it does

- Routes grouped by resource (`sandbox_routes`, `drive_routes`, `image_routes`,
  `pool_routes`, `snapshot_routes`, `websocket_routes`, `preview_routes`), each its
  own `Router` with its own auth middleware layer, merged into one `app` —
  `core/crates/daemon/src/main.rs`.
- `/healthz` and `/metrics` are the only unauthenticated routes.

## Key terms

- **Bearer token auth.** `Authorization: Bearer <token>` checked against
  `SANDKILN_AUTH_TOKEN`; a no-op (open access) when that env var is unset —
  `auth::require_bearer_token`. The actual string comparison is pulled into a
  small pure function (`auth::token_matches`) specifically so it's unit-testable
  without building a real `axum::Request`.
- **Preview token auth.** A second, separate middleware (`auth::require_preview_token`)
  for anything a *browser tab* has to hit directly — dev-server preview URLs and
  WebSocket upgrades (PTY, exec-stream log attach). Neither a plain navigation nor
  a native `WebSocket` constructor can set a custom header, so these additionally
  accept the token as a `?token=` query parameter.
- **Request-id correlation.** Every request gets an `X-Request-Id` (caller-supplied
  via header, or a generated UUID) attached as a `tracing::Span`, so every log line
  a request causes — including ones from deep inside `sandkiln-vmm`'s
  `spawn_blocking` calls — carries the same id. `request_id.rs`.
- **Structured tracing**, not `println!` logging — `tracing`/`tracing-subscriber`,
  pretty or JSON output, span-parented so a request's full call tree is
  reconstructable from logs alone.
- **Hand-rolled Prometheus metrics** (`metrics.rs`) — four metrics behind atomics
  and a small histogram implementation, deliberately not the `prometheus` crate:
  the exposition text format is a few lines of string formatting, not enough
  surface to justify a dependency and its registry abstraction. Tracks
  `sandboxes_created_total`, `boot_duration_ms`, `exec_latency_ms`, and a
  per-phase `create_phase_duration_ms` histogram family (rootfs clone, network
  lease, boot, etc.) used by `scripts/bench-report.sh` to catch regressions.

## Status

Done, live-verified. See `scripts/integration-test.sh`'s `13-auth.sh` and
`04-request-id.sh` topics for the exact behaviors exercised.

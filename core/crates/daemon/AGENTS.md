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
  subscriber is initialized.
- `metrics.rs` — `Metrics`: the `/metrics` endpoint's counters/gauge/
  histograms and a hand-rolled Prometheus text-exposition-format writer.
  Lives on `AppState` (`state.metrics`); route handlers record into it at
  the same call sites `sandkiln-vmm`'s `tracing` events fire from
  (`routes_sandbox::create_sandbox` for boot duration and the created
  counter, `routes_exec::call_agent` for exec latency). No metrics crate
  dependency — see the module doc comment for why.
- `auth.rs` — bearer-token middleware. No-ops entirely if
  `SANDKILN_AUTH_TOKEN` is unset.
- `state.rs` — `AppState`: the daemon's config, `NetworkManager`, and
  in-memory sandbox map (`Mutex<HashMap<String, Sandbox>>`). This map
  *is* the daemon's entire notion of sandbox state — it doesn't survive a
  restart (see `ROADMAP.md`'s sqlite-backed-state item).
- `sandbox.rs` — the `Sandbox` struct the daemon tracks per running VM
  (id, `Vm` handle, network `Lease`, rootfs path, tags, created-at).
- `routes_sandbox.rs` — sandbox lifecycle handlers: create/list/stop.
- `routes_exec.rs` — exec/read-file/write-file handlers. `call_agent()`
  is the shared helper all three use — extend it, don't duplicate its
  pattern.
- `routes_drives.rs` / `routes_snapshot.rs` — drives and snapshot/resume
  handlers, each in their own file for the same reason as above.
- `routes_metrics.rs` — the `/metrics` handler. Unauthenticated like
  `/healthz` (wired directly on `app` in `main.rs`, not through either
  auth-gated router) since it's operational data about the daemon, not
  sandbox data.
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

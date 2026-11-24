# Observability

| Feature | Detail |
| --- | --- |
| Structured logging | `tracing` throughout: HTTP method, path, status, latency via `tower-http` `TraceLayer`; VM lifecycle (boot with timing, vsock call latency, stop) from `sandkiln-vmm` itself so the library is useful standalone. Correlated by `vm_id`, filtered per module with `RUST_LOG`. |
| Request-id correlation | Every request gets an id (caller's `X-Request-Id` or generated) set as the active span before any handler, echoed in the response, and propagated across `spawn_blocking` into every vmm call it triggers (`tracing_util::spawn_blocking_in_current_span`). One id ties an HTTP request to the VM boot, call and stop lines it caused. |
| `/metrics` | Prometheus text format, unauthenticated like `/healthz`. `sandboxes_created_total`, `sandboxes_active`, boot and exec histograms, `create_phase_duration_ms{phase}`, `snapshot_take_phase_duration_ms{phase}`, `rootfs_clone_total{method}`. Hand-rolled exposition rather than a new dependency (`metrics.rs`). |
| JSON logs | `SANDKILN_LOG_FORMAT=json` for production pipelines; pretty terminal format by default. |
| Guest console capture | Firecracker's stdout/stderr (the guest's `console=ttyS0`) goes to a per-VM log file, and a boot failure's error message names the file. A guest that dies before answering over vsock (kernel panic, agent crash) is no longer invisible. |

Operator health: `GET /status` (with `kiln status` and SDK `getStatus`/
`get_status`) shipped. As designed it reports version, uptime, live, held and
retired counts, pools, tap pool total, free and in use, the sandbox IP pool with
how many addresses held snapshots keep (`NetworkManager::free_tap_count`,
`free_ip_count`), free bytes for `state_dir` and
`rootfs_dir`, `rootfs_clone_total`, the exec timeout, and the auth and
jailer flags, exposed through `AppState`. Remaining: `kiln doctor` and a
dashboard page on the same endpoint (a dashboard of sandboxes, pools, drives
and history), see the [roadmap](../../ROADMAP.md).

Request-id correlation, `/metrics` and the other items above are
live-verified.

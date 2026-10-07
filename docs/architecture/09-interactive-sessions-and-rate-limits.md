# PTY, exec-stream, and rate limiting

## Interactive terminals (PTY)

`routes_pty.rs` upgrades a WebSocket to a live, interactive shell session
inside the sandbox.

- **`forkpty(3)`** inside the guest agent (`nix::pty::forkpty`) — the standard
  POSIX call that allocates a pseudo-terminal pair and forks, with the child
  getting the pty slave as its controlling terminal. This is glibc's own
  well-trodden contract; the guest agent's job is just picking terminal
  dimensions (`Winsize`) and wiring the resulting fd pair to `PTY_PORT`'s raw
  byte passthrough (see [06](06-vsock-protocol.md)).
- Guarded by `auth::require_preview_token`, not `require_bearer_token` — a
  browser's native `WebSocket` can't set an `Authorization` header, so the
  `?token=` fallback applies here too (see [01](01-daemon-api-and-auth.md)).

## exec-stream (streamed long-running command output)

`routes_logs.rs` / `EXEC_STREAM_PORT` — start a command, then attach one or
more WebSocket "log" connections that receive its output as it's produced,
rather than waiting for it to finish and returning one blob (`AGENT_PORT`'s
`exec` behavior). Useful for anything long-running (a build, a dev server) an
agent or a UI wants to watch live. A session persists independent of any one
attached viewer — `list_exec_streams`/`attach_logs` are separate from starting
one.

## Rate limiting — token bucket, at the VM level

Not an API request-throttle — this is Firecracker's own **per-drive/per-VM
bandwidth and operations rate limiter**, exposed through sandkiln's
`RateLimiter`/`TokenBucket` types (`sandkiln-vmm::vm`) and the daemon's
`rate_limit` field on `POST /sandboxes`.

- **Token bucket algorithm**: a bucket refills by `size` tokens every
  `refill_time` milliseconds; each operation (or byte, for bandwidth) consumes
  a token; an empty bucket blocks until it refills. An optional
  `one_time_burst` lets an initial burst through before the steady refill rate
  applies.
- **Bandwidth and ops limits are independent** — either or both can be set;
  `resolve_rate_limit` (`routes_sandbox.rs`) rejects a `rate_limit` that sets
  neither sub-field, rather than silently accepting a no-op request.
- Like `drives`, a custom `rate_limit` is baked in at boot time — a request
  specifying one never matches a pre-warmed pool (see [02](02-vm-boot-and-latency.md)).

## Status

All three done, live-verified — `scripts/integration-test.sh`'s `17-pty.sh`,
`23-exec-stream-logs.sh`, `03-rate-limiting.sh`. PTY: JS/TS SDK + CLI only, not
yet Python. See [`website/src/content/docs/internals/pty.md`](../../website/src/content/docs/internals/pty.md),
[`exec-stream.md`](../../website/src/content/docs/internals/exec-stream.md), and
[`rate-limiting.md`](../../website/src/content/docs/internals/rate-limiting.md).

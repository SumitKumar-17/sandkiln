# Dev servers, preview, terminals and streamed exec

## Host-side reverse proxy

`ANY /sandboxes/:id/preview/:port[/path]` proxies a full HTTP request to a
server on that port inside the sandbox over the bridge. SDKs:
`Sandbox.previewUrl()`/`preview_url()`; CLI: `kiln sandbox preview`;
reference: `examples/dev-server-preview`. Preview routes accept the auth
token as `?token=` since the caller is usually a browser tab or iframe that
cannot set headers. Verified with a real `python3 -m http.server`, the
502/404/401 paths, and previewing a sandbox forked from a snapshot (which
borrows its network lease). WebSocket proxying (dev-server HMR) is scoped
out: plain HTTP only.

Alternative not built: a dedicated public domain per exposed port instead
of a `/preview/:port` path. It needs real DNS and routing infrastructure.
Also open: fast iterative file sync for dev-server workflows (many small
files, watch mode).

## Local tunnel (the reverse direction)

`sandbox.tunnel(guestPort, { localPort })` (JS/TS and Python, sync and
async) and `kiln sandbox tunnel <id> <guest-port> --local-port <port>` make
`127.0.0.1:guestPort` inside the sandbox be `127.0.0.1:localPort` on the
caller's machine.

It is the one exception to the "host connects in, guest only listens" vsock
invariant: a new `TUNNEL_PORT` where the guest connects out, using
Firecracker's guest-initiated mechanism (a pre-bound `<uds_path>_<port>`
Unix socket, not the `CONNECT <port>` handshake reversed, which an early
sketch wrongly assumed). Guest connections are multiplexed over one
WebSocket to the caller with a small binary frame. JS/TS uses the native
`WebSocket`; Python has none in its stdlib, so the SDK carries a hand-rolled
RFC 6455 client (handshake math checked against the RFC test vector),
its first WebSocket feature. Verified on all three surfaces with a real local
HTTP server reached by `curl` inside a sandbox, byte for byte.
Reference: `examples/local-tunnel`.

## Interactive terminal

`GET /sandboxes/:id/pty[?cols=&rows=]` upgrades to a WebSocket and proxies
raw bytes to a shell forked via `forkpty(2)` in the guest, over a dedicated
vsock port (`PTY_PORT`, separate from the request/response `AGENT_PORT`).
`Sandbox.pty()` (JS/TS, native `WebSocket`), `kiln sandbox pty <id>` (raw
terminal mode, Ctrl+C passes through), `examples/interactive-terminal`.
Not in the Python SDK.

- Hangup works both ways: shell exit ends the WebSocket; WebSocket close
  sends the shell's process group `SIGHUP`, so nothing runs orphaned.
  `17-pty.sh` checks a real command's output and the SIGHUP cleanup (via
  `scripts/lib/pty-check.mjs`, since the rest of the harness is bash+curl).
- Auth: `?token=` accepted, as neither browsers nor Node's `WebSocket` can
  set headers.
- Cap: `MAX_PTY_SESSIONS_PER_SANDBOX = 64`, daemon-side.
- No live resize: `cols`/`rows` apply once at open via `PtyHandshake`. A
  resize needs a control-message side channel over the same WebSocket.

## Streamed background exec

Distinct from batch `exec` (one blob back) and `pty` (a live shell): runs a
command detached in the guest and lets any number of callers attach over
time, each getting a replay of everything captured so far, then a live tail.

- `POST /sandboxes/:id/exec-stream` starts and returns a session id;
  `GET /sandboxes/:id/exec-stream` lists sessions (running or finished);
  `GET .../exec-stream/:id/logs` (WebSocket) attaches.
- A third vsock port (`EXEC_STREAM_PORT`) carries framed `ExecStreamEvent`s
  (stdout/stderr chunks, then one `Exit`) from a child with no controlling
  terminal. The daemon does the buffering (a bounded 1MiB ring plus a
  `tokio::sync::broadcast` fan-out in `log_session::LogSession`), not the
  guest, since capture must continue with no one watching.
- SDK and CLI: `execStream()`/`listExecStreams()`/`attachLogs()` (JS/TS),
  `kiln sandbox exec-stream <id> -- <cmd>` and `kiln sandbox logs <id>
  [session-id]`, both returning the remote exit code. Python has none.
- Verified: replay then live tail, instant full replay after finish, two
  concurrent attaches (one late) both getting the ordered log.
  `23-exec-stream-logs.sh`.
- Limits (v1 scope): not carried across resume, fork or restore and lost on
  daemon restart (daemon-process state, like `pty_session_count`). No
  kill endpoint, since the guest agent has no registry to signal a child by
  id. No concurrent-session cap, since sessions share nothing.

# Dev servers, preview, terminals and streamed exec

## Host-side reverse proxy

`ANY /sandboxes/:id/preview/:port[/path]` proxies a full HTTP request to a
server on that port inside the sandbox over the bridge. SDKs:
`Sandbox.previewUrl()`/`preview_url()`; CLI: `kiln sandbox preview`;
reference: `examples/dev-server-preview`. Preview routes accept the auth
token as `?token=` since the caller is usually a browser tab or iframe that
cannot set headers. Live-verified with a real `python3 -m http.server`, the
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

The mechanism (guest-initiated `TUNNEL_PORT` via a pre-bound
`<uds_path>_<port>` socket, one multiplexed WebSocket, the hand-rolled
Python RFC 6455 client checked against the RFC test vector) is on the
[local tunnel internals page](../../docs-site/internals/local-tunnel.mdx);
the guest-initiated-vsock discovery is in the
[engineering notebook](../../docs-site/architecture/engineering-notebook.mdx).
Not on those pages: an early design sketch wrongly assumed the
`CONNECT <port>` handshake reversed; this is the Python SDK's first
WebSocket feature. Live-verified on all three surfaces with a real local HTTP
server reached by a real `curl` run inside a real sandbox, byte for byte.
Reference: `examples/local-tunnel`.

## Interactive terminal

`GET /sandboxes/:id/pty[?cols=&rows=]` upgrades to a WebSocket and proxies
raw bytes to a shell forked via `forkpty(2)` in the guest, over a dedicated
vsock port (`PTY_PORT`, separate from the request/response `AGENT_PORT`).
`Sandbox.pty()` (JS/TS, native `WebSocket`), `kiln sandbox pty <id>` (raw
terminal mode, Ctrl+C passes through), `examples/interactive-terminal`.
Not in the Python SDK.

- Mechanism (dedicated `PTY_PORT`, `PtyHandshake`, SIGHUP to the process
  group on a WebSocket close, raw-mode CLI): see the
  [PTY internals page](../../docs-site/internals/pty.mdx). Live-verified end
  to end in both hangup directions. `17-pty.sh` checks a real command's
  output round-tripping through a real shell and the SIGHUP cleanup (open a
  session, disconnect without exiting the shell, `exec` a `ps` check
  confirming nothing orphaned survives; via `scripts/lib/pty-check.mjs`,
  since the rest of the harness is bash+curl).
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
- Mechanism (`EXEC_STREAM_PORT`, framed `ExecStreamEvent`s, daemon-side
  `LogSession` ring buffer plus broadcast fan-out): see the
  [exec-stream internals page](../../docs-site/internals/exec-stream.mdx).
  The ring is bounded at 1MiB. The daemon buffers, not the guest, since
  capture must continue with no one watching.
- SDK and CLI: `execStream()`/`listExecStreams()`/`attachLogs()` (JS/TS),
  `kiln sandbox exec-stream <id> -- <cmd>` and `kiln sandbox logs <id>
  [session-id]`, both returning the remote exit code. Python has none.
- Live-verified: a multi-line, multi-second command; attach mid-stream sees
  replay then live tail; reattach after finish sees a full instant replay;
  two concurrent attaches to one session, one joining late, both get the
  complete ordered log. Also: replay then live tail, instant full replay after finish, two
  concurrent attaches (one late) both getting the ordered log.
  `23-exec-stream-logs.sh`.
- Limits (v1 scope): not carried across resume, fork or restore and lost on
  daemon restart (daemon-process state, like `pty_session_count`). No
  kill endpoint, since the guest agent has no registry to signal a child by
  id. No concurrent-session cap, since sessions share nothing.

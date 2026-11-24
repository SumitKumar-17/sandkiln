# sandkiln interactive terminal

A minimal reference example of interactive terminal access: open a live,
bidirectional shell session inside an isolated sandkiln sandbox from your
own terminal, using the published `sandkiln` npm package's
`Sandbox.pty()` — not a toy snippet.

## What it does

1. Creates a sandbox with `Sandbox.create()`.
2. Opens a real, interactive shell inside it with `sandbox.pty()` — this
   returns a plain `WebSocket`, not a promise of one result; the session
   stays open until the remote shell exits (or the connection drops).
3. Puts this process's own stdin into raw mode and wires it straight to
   the WebSocket, and writes everything the shell sends back straight to
   stdout — the same shape `kiln sandbox pty` itself uses internally.
4. On `exit`/Ctrl+D (the shell ending, which closes the WebSocket) the
   example destroys the sandbox with `sandbox.stop({ keep: false })`.
   Ctrl+C is delivered to the *remote* shell, not this process — raw mode
   means Node never intercepts it.

See `index.js` — it's the whole program.

## Requirements

A running `sandkilnd` daemon reachable from this machine — see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the repo root for how to
stand one up. There is no hosted service. Needs Node.js >= 22 (a global
`WebSocket`) and a real TTY to run interactively (piping stdin still works
for scripted input, just without raw-mode keystroke passthrough).

## Run it

```
cd examples/interactive-terminal
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one. When set, `Sandbox.pty()` appends it to the
  WebSocket URL as a `?token=` query parameter automatically, since
  neither browsers' nor Node's native `WebSocket` constructor can set a
  custom `Authorization` header.

## How it was verified

Without a person at a keyboard, by running it under a pseudo-terminal
(`script -qec "node index.js" /dev/null`) with scripted keystrokes:
raw mode engaged, a command round-tripped (`tty` printed `/dev/pts/0`
inside the guest), Ctrl+C interrupted a `sleep 30` in the remote shell
without ending the session, and `exit` stopped and removed the sandbox.
`scripts/dev.sh examples-smoke` repeats that check. Not verified: how it
feels in a real terminal emulator (colors, full-screen programs such as
`vim`, window resizing, which is a known limitation below).

## Known limitations

- No live resize — `cols`/`rows` size the session once, at open time.
  Resizing your terminal after that won't reach the remote shell.
- A per-sandbox concurrent-session cap (64) is enforced by the daemon;
  irrelevant for this single-session example, but worth knowing if
  you're scripting many sessions against one sandbox.

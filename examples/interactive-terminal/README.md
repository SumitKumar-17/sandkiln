# sandkiln interactive terminal

Open a live shell inside a sandbox from your own terminal with `Sandbox.pty()`, the same way `kiln sandbox pty` does.

## What it does

1. Creates a sandbox and opens a shell with `pty()`, which returns a `WebSocket` that stays open until the shell exits.
2. Puts stdin in raw mode, wires it to the socket and writes the shell's output to stdout.
3. On `exit`/Ctrl+D, or Ctrl+C (delivered to the remote shell, since raw mode stops Node intercepting it), calls `stop()`.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service. It needs a real TTY to be interactive; piped stdin works without raw-mode passthrough.

## Run it

```
cd examples/interactive-terminal
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- With an auth token set, `pty()` appends it as `?token=` because `WebSocket` cannot set headers.
- No live resize: `cols` and `rows` apply once at open.
- The daemon caps concurrent sessions at 64 per sandbox.

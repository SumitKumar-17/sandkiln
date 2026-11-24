# sandkiln exec-stream logs

Start a command detached in a sandbox, watch it live, then reattach after it finished and get the whole log back, via `execStream()`, `listExecStreams()` and `attachLogs()`.

## What it does

1. Creates a sandbox and starts a roughly 6 second multi-line command with `execStream()`, which returns a session id immediately.
2. Attaches with `attachLogs(sessionId)` (a plain `WebSocket`) and prints lines as they arrive, ending with `[process exited with code 0]`.
3. Lists the session with `listExecStreams()`, now showing its exit code.
4. Reattaches to the finished session and gets the identical log in milliseconds. The daemon captures output from the start whether or not anything is attached, so each attach is a full replay then a live tail.
5. Calls `stop({ keep: false })`.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service.

Node.js 22 or newer: `attachLogs()` needs a global `WebSocket`.

## Run it

```
cd examples/exec-stream-logs
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- With an auth token set, `attachLogs()` appends it as `?token=` because `WebSocket` cannot set headers.
- Sessions live in daemon memory and are not carried across a restart, `resume()`, `fork()` or restore.
- There is no cancel endpoint; a session runs until its command exits or the sandbox stops.
- The replay buffer is capped; a longer log starts with a `[... N earlier bytes truncated ...]` notice.

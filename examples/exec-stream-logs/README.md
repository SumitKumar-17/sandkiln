# sandkiln exec-stream logs

A minimal reference example of streamed background exec sessions: start a
command running detached inside a sandbox, watch it live, then reattach
to the same session *after it has already finished* and get the whole log
back — via `Sandbox.execStream()`/`.listExecStreams()`/`.attachLogs()`.

> **This example points at the in-repo SDK source (`packages/sdk`), not
> the published `sandkiln` npm package**, because
> `execStream`/`listExecStreams`/`attachLogs` haven't been published yet.
> Every other example here depends on the published package, as
> `examples/AGENTS.md` requires — this one deliberately deviates until a
> new version ships, and should be switched back to the published
> `sandkiln` dependency once it does.

## What it does

1. Creates a sandbox with `Sandbox.create()`.
2. Starts a ~6-second, multi-line command with
   `sandbox.execStream(command, args)`, which returns a session id
   **immediately** — unlike `runCommand()`, it does not hold an HTTP
   request open for the command's lifetime.
3. Attaches with `sandbox.attachLogs(sessionId)` — a plain `WebSocket`,
   the same shape `Sandbox.pty()` returns — and prints each line as it
   arrives, ending with the daemon's `[process exited with code 0]`
   notice.
4. Lists the session with `sandbox.listExecStreams()`, now showing its
   exit code.
5. **Reattaches to the same, already-finished session** and prints what
   it gets: byte-for-byte the same log, in milliseconds instead of
   seconds. This is the distinctive part — the daemon captures the
   output from the moment the command starts, independently of whether
   anything is attached, so every attach is a full replay followed by a
   live tail rather than "whatever happens after you connect".
6. Stops the sandbox with `sandbox.stop({ keep: false })`.

See `index.js` — it's the whole program.

## Requirements

A running `sandkilnd` daemon reachable from this machine — see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the repo root for how to
stand one up. There is no hosted service.

Node.js >= 22: `attachLogs()` needs a global `WebSocket`, same as
`pty()`.

## Run it

Because this example uses the in-repo SDK (see the note above), build it
once from the repo root first:

```
npm install
npm run build
```

Then:

```
cd examples/exec-stream-logs
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one. When set, `attachLogs()` appends it to the WebSocket
  URL as a `?token=` query parameter automatically, since neither
  browsers' nor Node's native `WebSocket` constructor can set a custom
  `Authorization` header.

## Known limitations

- Sessions live in the daemon's memory: they don't survive a daemon
  restart, and aren't carried across `resume()`/`fork()`/a restored
  checkpoint. The same scope PTY sessions already have.
- No kill/cancel endpoint yet — a background session runs until its
  command exits or the sandbox stops.
- The replay buffer is capped; an attach to a session that has already
  produced more output than the cap starts with a
  `[... N earlier bytes truncated ...]` notice.

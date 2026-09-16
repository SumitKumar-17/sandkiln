# sandkiln named persistent sandbox

A minimal reference example of named sandboxes and persistent-by-default
stop: `Sandbox.getOrCreate({ name })` resolves a name to a sandbox,
creating or resuming as needed, and a plain `sandbox.stop()` keeps its
state instead of destroying it — so a caller that only ever deals in
names, never ids, gets a workspace that survives being stopped between
turns — using the published `sandkiln` npm package, not a toy snippet.

## What it does

1. Calls `Sandbox.getOrCreate({ name: "example-counter-agent" })`, reads
   a run counter file from inside the sandbox (0 if this is the first
   time), increments it, writes it back, and stops the sandbox with the
   *default* options — no `{ keep: false }`.
2. Calls `Sandbox.getOrCreate` again with the same name. Since the prior
   step's `stop()` kept state, this resumes it rather than creating
   fresh, and the counter picks up where it left off.
3. Does a final `getOrCreate` and this time destroys it for real with
   `stop({ keep: false })`, so the example doesn't leave a snapshot
   behind under this name forever.

See `index.js` — it's the whole program. Steps 1 and 2 are written as two
simulated "process invocations" inside one script so the persistence
claim is provable from a single run, but running `node index.js` twice
in a row, as two genuinely separate processes, produces the identical
behavior — that's the actual point.

## Why this matters

Most sandbox platforms treat "stop" as "gone" — you keep your own
database row mapping some external id to a sandbox id, and a stop means
starting over next time. sandkiln's default is the other way around:
`stop()` snapshots and retires the id, but the *name* keeps working, so
the caller doesn't need a separate persistence layer just to remember
"the workspace for user 42" or "yesterday's agent session" across
restarts. Passing `{ keep: false }` still gets the old
"just destroy it" behavior for a sandbox you genuinely never want back.

## Requirements

A running `sandkilnd` daemon reachable from this machine — see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the repo root for how to
stand one up. There is no hosted service.

## Run it

```
cd examples/named-persistent-sandbox
npm install
node index.js
```

Run it again afterward (`node index.js` a second time) to see it resume
the same named sandbox across a genuinely separate process.

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one.

See `ROADMAP.md`'s "Persistence and snapshotting" section at the repo
root for the full design, including how this interacts with
auto-suspend (an idle named sandbox can be snapshotted by the daemon on
its own, with the name still resolving to it afterward).

# sandkiln snapshot lifecycle

A minimal reference example of `snapshot()`/`fork()`/`resume()`: pay an
expensive setup cost once, freeze the result, then boot two independent
branches from the identical frozen state without repeating the setup —
using the published `sandkiln` npm package, not a toy snippet.

## What it does

1. Creates a sandbox and does a one-time setup step (`writeFile`, standing
   in for whatever's actually expensive to redo: installing dependencies,
   downloading a dataset, warming a cache).
2. Freezes it with `sandbox.snapshot()` — the sandbox's memory and disk
   are saved to disk and the original sandbox id is retired.
3. Forks branch A from the snapshot with `Sandbox.fork(snapshotId)`,
   confirms it sees the base state, writes a marker file unique to this
   branch, then stops it.
4. Forks branch B from the *same* snapshot, confirms it also sees the
   base state, and confirms branch A's marker file did **not** leak into
   it — each fork is an independent branch off the same frozen point, not
   a shared instance.
5. Resumes the snapshot with `Sandbox.resume(snapshotId)` and confirms
   the snapshot is now gone — unlike `fork()`, `resume()` consumes it,
   so a second `fork()`/`resume()` call against the same id fails.

See `index.js` — it's the whole program.

## Fork vs. resume, the actual distinction this example proves

Both boot a new sandbox from a snapshot's exact frozen state. The
difference is what happens to the snapshot afterward: `resume()` retires
it (one new sandbox, then the snapshot is gone), `fork()` leaves it
usable again once the forked sandbox stops (many independent branches,
each starting from the identical state, none of them affecting each
other or the snapshot itself). Only one live fork of a given snapshot
can exist at a time — a second `fork()` call while an earlier fork is
still running rejects with a 409, which is why this example stops branch
A before forking branch B.

## Requirements

A running `sandkilnd` daemon reachable from this machine — see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the repo root for how to
stand one up. There is no hosted service.

## Run it

```
cd examples/snapshot-lifecycle
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one.

See the [Snapshots, resume, and fork](../../website/src/content/docs/concepts/snapshots.md)
docs page, and `ROADMAP.md`'s "Persistence and snapshotting" section at
the repo root, for the full design — including the pre-warmed pool
feature this same primitive underpins (see `pool-warm-start/`).

# sandkiln snapshot lifecycle

`snapshot()`, `fork()` and `resume()`: pay a setup cost once, freeze the result, then boot independent branches from the same state.

## What it does

1. Creates a sandbox and does a one-time setup (`writeFile`, standing in for installing dependencies or downloading data).
2. Freezes it with `snapshot()`; the original id is retired.
3. Forks branch A with `Sandbox.fork(snapshotId)`, checks it sees the base state, writes a marker file, then stops it.
4. Forks branch B from the same snapshot and checks the base state is there and A's marker is not.
5. Resumes with `Sandbox.resume(snapshotId)` and confirms the snapshot is now gone.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service.

## Run it

```
cd examples/snapshot-lifecycle
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- `resume()` consumes the snapshot; `fork()` leaves it reusable.
- Only one live fork of a snapshot may exist at a time (a second gets `409`), which is why branch A stops before B starts.
- See [Snapshots, resume, and fork](https://sandkiln.mintlify.site/concepts/snapshots).

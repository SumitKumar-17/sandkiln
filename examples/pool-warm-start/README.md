# sandkiln pool warm start

Pre-warmed pools: configure a pool with `Pool.create()`, wait for a warm snapshot, then claim from it repeatedly and report what happens each time.

## What it does

1. Configures a pool with `Pool.create(id, { warmCount: 1 })`.
2. Five times: polls `Pool.list()` until a warm snapshot is ready, then times a plain `Sandbox.create()`. With no drives or rate limit and matching resources it resumes the warm snapshot; there is no separate claim call.
3. Classifies each attempt as a clean claim or a fallback (the resumed snapshot failed its post-resume health check and the daemon did a normal cold create instead).
4. Prints the counts and average clean-claim latency, then deletes the pool.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service.

## Run it

```
cd examples/pool-warm-start
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- Several attempts because resuming has a real Firecracker/KVM failure mode on the dev box (the restored guest kernel can panic early; roughly 1 in 3 to 2 in 3 resumes). A single run would mislead either way. See `ROADMAP.md`.
- No `maxCount` is set, so a claim with nothing warm falls through to a cold create. `Pool.create(id, { maxCount })` adds a ceiling with queueing.
- Pool configuration is in daemon memory only; re-run `Pool.create()` after a restart.

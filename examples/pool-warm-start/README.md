# sandkiln pool warm start

A minimal reference example of pre-warmed pools: configure a pool with
`Pool.create()`, wait for a background replenisher to prepare a warm,
resumable snapshot, then claim from it several times in a row and report
what actually happens each time — using the published `sandkiln` npm
package, not a toy snippet, and not a single cherry-picked run.

## What it does

1. Configures a pool with `Pool.create(id, { warmCount: 1 })`.
2. Repeats, 5 times: polls `Pool.list()` until the pool reports a warm,
   ready-to-resume snapshot (replenishment happens in the background and
   takes a few seconds — a real cold boot + snapshot, not instantly),
   then times a plain `Sandbox.create()`. Since it has no
   `drives`/`rateLimit` and its (default) image/resources match the
   pool, this automatically resumes the warm snapshot instead of
   cold-booting — there's no separate "claim from pool" call; matching is
   entirely transparent.
3. Classifies each attempt as a **clean claim** (fast) or a **fallback**
   (the resumed snapshot failed a real post-resume health check, and the
   daemon transparently recovered with a normal cold create instead of
   ever handing back a broken sandbox — see "Why several attempts, not
   one" below).
4. Prints a summary: how many of each, and the average clean-claim
   latency.
5. Deletes the pool (which also cleans up anything it still has warm).

See `index.js` — it's the whole program.

## Why several attempts, not one

Resuming a snapshot has a real, currently **not rare** Firecracker/KVM
failure mode: the restored guest kernel can panic early in boot (an
early-boot divide-by-zero trap, most likely restored CPU/timer state
interacting badly with timing-sensitive init code) and Firecracker exits
shortly after. Measured directly, repeatedly, on the dev box this
feature was built on: roughly **1 in 3 to 2 in 3** resumes failed this
way across clean, isolated test runs — high enough that a single-attempt
demo would be misleading either direction depending on luck. Every claim
runs a real health check before being handed back, and safely falls back
to a normal cold create on failure — this example surfaces both
outcomes honestly instead of hiding one. See `ROADMAP.md`'s "Persistence
and snapshotting" section at the repo root for the full finding.

## Requirements

A running `sandkilnd` daemon reachable from this machine — see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the repo root for how to
stand one up. There is no hosted service.

## Run it

```
cd examples/pool-warm-start
npm install
node index.js
```

## Configuration

- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one.

## Known limitations

- No `maxCount` ceiling or queueing yet — a claim that arrives while
  nothing is warm just falls through to a normal cold create, unbounded.
- Pool configuration lives only in the daemon's memory, not durable
  across a restart — re-run `Pool.create()` afterward if you need it back.

See `ROADMAP.md`'s "Persistence and snapshotting" section at the repo
root for the full design, including a related finding (MMDS not
surviving snapshot/restore) not covered by this example.

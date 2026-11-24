# sandkiln named persistent sandbox

`Sandbox.getOrCreate({ name })` resolves a name to a sandbox, and a plain `stop()` keeps its state, so a caller that only deals in names gets a workspace that survives being stopped.

## What it does

1. Calls `getOrCreate({ name: "example-counter-agent" })`, reads a counter file in the sandbox (0 on first use), increments it and stops with default options.
2. Calls `getOrCreate` again with the same name. It resumes the stopped sandbox and the counter continues.
3. Does a final `getOrCreate` and destroys it with `stop({ keep: false })` so no snapshot is left under that name.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service.

## Run it

```
cd examples/named-persistent-sandbox
npm install
node index.js
# run it twice to see a resume across separate processes
```

## Configuration

- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- `stop()` snapshots and retires the id but the name keeps working, so no separate persistence layer is needed to remember a workspace. `{ keep: false }` destroys instead.
- An idle named sandbox can be auto-suspended by the daemon; the name still resolves to it.

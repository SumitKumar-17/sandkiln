# AGENTS.md — sandkiln (JS/TS SDK)

Read root `AGENTS.md` first.

## What this package is

Published JS/TS client: [`sandkiln` on npm](https://www.npmjs.com/package/sandkiln).
A thin, fully-typed wrapper over `sandkiln-daemon`'s HTTP API — never
implements logic the daemon doesn't already have; new behavior goes in
`core/crates/daemon` first.

## Files

- **`sandbox.ts`** — `Sandbox`: static `create`/`attach`/`list`/`resume`/
  `fork`/`byName`/`getOrCreate`; instance `runCommand`/`readFile`/
  `writeFile`/`chmod`/`chown`/`mkdir`/`rename`/`copy`/`symlink`/
  `readlink`/`truncate`/`listDir`/`stop`/`snapshot`/`previewUrl`/`pty`/
  `execStream`/`listExecStreams`/`attachLogs`/`tunnel`. `pty`/`attachLogs`
  return a native `WebSocket` directly instead of awaiting (live streams,
  not a single call); both throw a clear error if `WebSocket` is
  undefined rather than adding a `ws` dependency (keeps zero-runtime-deps
  intact). `tunnel` is async (it POSTs to create the tunnel first) and
  returns a `TunnelHandle` — see `tunnel.ts`.
  `chmod`/.../`listDir` all forward paths unvalidated, same as
  `readFile`/`writeFile` (validation belongs daemon/guest-agent side).
  `resume`/`fork`/`byName` are static since none acts on an existing
  `Sandbox`; `fork` doesn't consume the snapshot, `resume` does (at most
  one live fork per snapshot — see daemon's `routes_snapshot.rs`).
  `getOrCreate` returns `{ sandbox, created }`. `stop()` returns
  `{ kept, snapshotId }` (daemon preserves by default; `{ keep: false }`
  for the old destroy behavior). `previewUrl` is pure/network-free,
  appends the auth token as `?token=` (browser tab/`<iframe>` caller).
  `CreateSandboxOptions.imageId` boots from a registered image.
- **`image.ts` / `drive.ts` / `pool.ts`** — static namespaces
  (`register|create`/`list`/`delete`), no instance behavior. `image.ts`
  mirrors the daemon's response shape including
  `guestAgentVerified`/`verificationHint` (always `false`, the daemon
  can't self-verify). Drive attachment happens via
  `CreateSandboxOptions.drives`, not through `Drive`. `Pool.create` takes
  a caller-given id (config handle, never guest-visible); no "claim"
  method exists — claiming is transparent, done by a plain
  `Sandbox.create()` matching a configured pool (`daemon/src/pool.rs`).
- **`client.ts`** — `ClientContext`/`resolveClient`, shared `baseUrl`/
  `authToken` resolution (pulled out once `image.ts` needed it too).
- **`http.ts`** — the one `fetch` call site. Non-2xx throws
  `SandkilnApiError`; empty body (incl. 204) resolves `undefined`.
- **`config.ts`** — env fallback (`SANDKILN_DAEMON_URL`/
  `SANDKILN_AUTH_TOKEN`), guards `process` existing at all.
- **`base64.ts`** — `Buffer`-or-`atob`/`btoa` fallback, same portability
  reasoning as `config.ts`.
- **`types.ts`** — wire shapes matching the daemon's JSON exactly
  (`snake_case` fields); public types (`ExecResult`, `SandboxInfo`,
  `ImageInfo`) translate to `camelCase`.
- **`tunnel.ts`** — `openTunnel`/`TunnelHandle`/`TunnelOptions`. Node-only
  (dynamically `import("node:net")` rather than a top-level import, so
  the module still loads — just can't be called — in a browser bundle):
  forwarding to a real local TCP socket needs `node:net`, which doesn't
  exist in a browser, unlike `pty`/`attachLogs` which only need a
  WebSocket. POSTs `/sandboxes/:id/tunnel` for a `tunnel_id`, opens the
  `.../ws` WebSocket (`?token=` fallback, same reason as `pty` — no
  WebSocket constructor in any runtime, browser or Node, can set a custom
  header), then demuxes/muxes a small binary frame (`op` byte + `conn_id`
  length + `conn_id` + payload) against real `net.Socket`s per
  `conn_id`. This exact frame format is shared with the daemon's
  `routes_tunnel.rs` and the Python SDK's `tunnel.py`/`atunnel.py` —
  change one, change all three.

## Testing

`test/previewUrl.test.js` (`node:test`, no added dep) covers the one
genuinely pure piece of logic here — everything else needs a live
daemon. Imports built `../dist/index.js`, so `npm test` runs `pretest`
(build) first. Run: `npm run test -w sandkiln`.

## Building and verifying

`npm run typecheck -w sandkiln && npm run build -w sandkiln` — necessary,
not sufficient: this SDK's real bugs have typechecked cleanly and only
shown up against a live daemon (wrong status-code assumption). Verify
against a real `sandkilnd`: sync to the dev box, start it, port-forward
(`ssh -f -N -L 7777:127.0.0.1:7777 <dev-box>`), run a script against
`dist/index.js` locally.

## Non-obvious things

- `kiln` depends on this package's built `dist/`, not source — CI builds
  this first. A public-API change breaks `kiln`'s build until rebuilt;
  expected, not a `kiln` bug.
- `Sandbox.attach(id, options)` does zero network calls by design (built
  for `kiln`'s case: a fresh process with only an id) — don't add
  validation that would require a round-trip.
- Published via `npm publish --provenance` in CI
  (`publish-sdk.yml`) — needs an npm token specifically marked to bypass
  2FA; a plain valid token gets rejected.

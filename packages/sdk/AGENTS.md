# AGENTS.md: sandkiln (JS/TS SDK)

Read root `AGENTS.md` first. The published client, [`sandkiln` on npm](https://www.npmjs.com/package/sandkiln):
a typed, thin wrapper over the daemon's HTTP API. It never implements logic the daemon lacks; new behavior goes
in `core/crates/daemon` first. Zero runtime dependencies.

## Files (`src/`)

- **`sandbox.ts`**: `Sandbox`. Statics: `create`, `attach`, `list`, `resume`, `fork`, `byName`,
  `getOrCreate` (returns `{ sandbox, created }`). Instance: `runCommand`, `readFile`, `writeFile`, filesystem ops
  (`chmod` to `listDir`), `stop` (returns `{ kept, snapshotId }`; `{ keep: false }` destroys), `snapshot`,
  `previewUrl`, `pty`, `execStream`, `listExecStreams`, `attachLogs`, `tunnel`, mounts. `pty` and `attachLogs`
  return a native `WebSocket` and throw a clear error if it is undefined, rather than adding a `ws` dependency.
  `previewUrl` is pure and appends the token as `?token=`. Paths are forwarded unvalidated (validation is
  daemon and guest side).
- **`image.ts`, `drive.ts`, `pool.ts`**: static namespaces. `Image` mirrors `guestAgentVerified` and
  `verificationHint`. Drives attach through `CreateSandboxOptions.drives`. There is no `Pool.claim`; claiming is
  transparent on a matching `Sandbox.create()`.
- **`status.ts`**: `getStatus`, host health and capacity (`GET /status`), backs `kiln status`.
- **`client.ts`**: `resolveClient`, shared `baseUrl`/`authToken` resolution. **`config.ts`**: env fallback
  (`SANDKILN_DAEMON_URL`, `SANDKILN_AUTH_TOKEN`), guards a missing `process`. **`http.ts`**: the one `fetch`
  call site; non-2xx throws `SandkilnApiError` (`errors.ts`), an empty body resolves `undefined`.
  **`base64.ts`**: `Buffer` or `atob`/`btoa`. **`types.ts`**: wire shapes (`snake_case`) and public camelCase types.
- **`tunnel.ts`**: `openTunnel`, `TunnelHandle`. Node only (dynamic `import("node:net")` so the module still loads in a
  browser bundle). POSTs for a `tunnel_id`, opens the `.../ws` WebSocket (`?token=` fallback), then muxes a small binary
  frame (`op`, `conn_id` length, `conn_id`, payload) against local sockets. The frame format is shared with the
  daemon's `http/routes/tunnel.rs` and the Python `tunnel.py`/`atunnel.py`: change one, change all three.

## Invariants

- `Sandbox.attach(id)` makes zero network calls (built for the CLI's fresh-process case); do not add validation
  that needs a round trip.
- `kiln` depends on this package's built `dist/`, so a public API change breaks the CLI build until rebuilt.

## Testing and release

`npm run typecheck -w sandkiln && npm run build -w sandkiln`, then `npm run test -w sandkiln` (`node:test`; `pretest`
builds; `test/previewUrl.test.js` covers the one pure piece). Typechecking is not proof: verify against a real
`sandkilnd` (port-forward the dev box's 7777 and run a script against `dist/index.js`). CI publishes with
`npm publish --provenance` (`publish-sdk.yml`); the npm token must be marked to bypass 2FA.

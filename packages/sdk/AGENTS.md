# AGENTS.md — sandkiln (JS/TS SDK)

Read the root `AGENTS.md` first for project-wide conventions. This file
is scoped to this one package.

## What this package is

The published JS/TS client: [`sandkiln` on
npm](https://www.npmjs.com/package/sandkiln). A thin, fully-typed
wrapper over `sandkiln-daemon`'s HTTP API — it should never contain logic
the daemon doesn't already implement. If a method here would need new
server-side behavior, that behavior goes in `core/crates/daemon` first;
this package only ever catches up to what the daemon actually does.

## Files

- `sandbox.ts` — the `Sandbox` class: `create`/`attach`/`list`/`resume`/
  `fork` (static), `runCommand`/`readFile`/`writeFile`/`stop`/`snapshot`/
  `previewUrl` (instance). Every instance method reuses the
  `baseUrl`/`authToken` the sandbox was created/attached with — see the
  `ClientContext` pattern. `resume`/`fork` are static (not instance
  methods) because neither acts on an already-existing `Sandbox` — they
  boot a *new* one from a snapshot id, the same shape as `create`. `fork`
  doesn't consume the snapshot, `resume` does; see the daemon's
  `routes_snapshot.rs` module doc comment for why at most one live fork of
  a given snapshot can run at a time. `previewUrl` is pure and
  network-free like `attach` — it just builds the URL for the daemon's
  `/sandboxes/:id/preview/:port` reverse proxy (see
  `core/crates/daemon/src/routes_preview.rs`), appending the auth token as
  a `?token=` query parameter rather than a header when one is configured,
  since the caller is typically a browser tab or `<iframe>`.
- `http.ts` — the one place `fetch` gets called. Non-2xx responses throw
  `SandkilnApiError`; a 204 (or any empty body) resolves to `undefined`.
  If the daemon's status-code contract ever doesn't match what's handled
  here, that's a real bug worth fixing on whichever side is wrong — see
  the `DELETE` 200-vs-204 story in root `AGENTS.md`.
- `config.ts` — env var fallback resolution (`SANDKILN_DAEMON_URL`,
  `SANDKILN_AUTH_TOKEN`). Guards `process` existing at all, since this
  package could theoretically load somewhere without it.
- `base64.ts` — `Buffer`-or-`atob`/`btoa` fallback, same portability
  reasoning as `config.ts`.
- `types.ts` — every request/response shape, matching the daemon's JSON
  exactly (including its `snake_case` fields like `content_base64`,
  `exit_code`) — the public-facing types (`ExecResult`, `SandboxInfo`)
  translate those into idiomatic `camelCase`.

## Testing

`test/previewUrl.test.js` covers `Sandbox.previewUrl`'s pure URL-building
logic with `node:test` (no added dependency — same convention as `kiln`'s
`test/format.test.js`). It imports the built `../dist/index.js`, not TS
source directly, so `npm test` runs `pretest` (`npm run build`) first.
Every other instance method needs a live daemon to test meaningfully (see
"Building and verifying" below), which is why this is the only file here
today — it's specifically the one piece of genuinely pure logic in this
package. Run: `npm run test -w sandkiln`.

## Building and verifying

```
npm run typecheck -w sandkiln
npm run build -w sandkiln
```
Typechecking and building are necessary but not sufficient — this SDK's
own bugs have specifically been the kind that typecheck cleanly (wrong
assumption about a status code) and only show up against a live daemon.
**Verify against a real `sandkilnd`** before calling a change done: sync
to the dev box, start the daemon, port-forward its port
(`ssh -f -N -L 7777:127.0.0.1:7777 <dev-box>`), then run a small script
against `dist/index.js` locally. See `git log` for prior examples of
exactly this pattern.

## Non-obvious things specific to this package

- **`kiln` (the CLI) depends on this package as a real workspace
  dependency resolved through its built `dist/`, not its source.** CI
  builds this package before typechecking/building `kiln` for exactly
  this reason — see the CI-ordering bug in root `AGENTS.md`. If you
  change this package's public API, `kiln`'s build will fail until it's
  rebuilt too; that's expected, not a bug in `kiln`.
- **`Sandbox.attach(id, options)` exists specifically for `kiln`'s use
  case** (a fresh CLI process that only has an id from a previous
  invocation, no live `Sandbox` object to hold onto) — it does zero
  network calls, it just constructs a handle. Don't add validation here
  that would require a round-trip; that defeats the point.
- Published with `npm publish --provenance` from CI
  (`.github/workflows/publish-sdk.yml`), not manually — see that
  workflow's comments for what's needed to re-publish (an npm token
  specifically marked to bypass 2FA; a plain token, even a valid one,
  gets rejected — this cost real back-and-forth to figure out).

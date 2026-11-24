# AGENTS.md — kiln (CLI)

Read root `AGENTS.md` first.

## What this package is

`kiln`: a thin `commander`-based CLI wrapping `packages/sdk` — manual
testing, agentic workflows, debugging without writing code. Essentially
no logic of its own beyond argument parsing/output formatting; every
operation delegates to the SDK.

## Files

- **`src/index.ts`** — entry point only: builds `program` (name,
  description, global `--base-url`/`--token`), calls each
  `commands/*.ts`'s `register*Commands(program)`, then
  `program.parseAsync(...)` with a `.catch()` backstop.
- **`src/commands/shared.ts`** — cross-group helpers: `GlobalOptions`,
  `clientOptions` (resolves `--base-url`/`--token`, falls through to the
  SDK's env resolution when unset), `fail`/`handleApiError` (clean
  `error: ...` on stderr + exit 1, never a raw stack trace).
- **`src/commands/sandbox.ts`** — the large group: `sandbox
  create|get-or-create|by-name|ls|rm|exec|read|write|preview|pty|tunnel|
  exec-stream|logs|snapshot|snapshots|resume|fork|chmod|chown|mkdir|
  rename|cp|symlink|readlink|truncate|ls-dir|mount|mounts|unmount`, each
  a thin call into `Sandbox`/`Sandbox.attach()`. Also owns
  `tagOption`/`envOption`/`buildEgressOption` and local helpers
  `attachSandbox` (reconstructs a handle from an id, no round-trip) and
  `followLogs` (replay-then-live-tail, shared by `exec-stream`/`logs`).
  - `pty <id>`: raw stdin mode so every keystroke (incl. Ctrl+C) goes
    straight to the remote shell. Real gotcha already found: a 10s hang
    after shell exit was a guest-side bug (`close` never fired until the
    guest tore down its side — see `sandkiln-guest-agent/pty.rs`), not a
    client leak — check the server side first if this recurs.
  - `tunnel <id> <guest-port> --local-port <port>`: runs in the
    foreground until `Ctrl+C`/`SIGTERM`, which calls `handle.close()`
    then exits — no raw-stdin mode needed (unlike `pty`), since this
    forwards bytes between the daemon's WebSocket and a real local
    socket, not a terminal.
  - `exec-stream`/`logs` resolve exit code by parsing the daemon's
    bracketed `[process exited with code N]` notice out of plain output.
  - `resume`/`fork`/`get-or-create`/`by-name` call the SDK's static
    methods directly (no existing handle to attach to). `create --image
    <id>` boots from a registered image. `rm` defaults to persist
    (`stop()`), reports kept-with-snapshot-id vs. destroyed; `--destroy`
    forces `stop({ keep: false })`. `preview` makes no network call —
    prints `previewUrl()`'s pure result; validation lives in the SDK.
- **`src/commands/image.ts` / `drive.ts`** — thin calls into
  `Image`/`Drive` statics (no instance handle, so no `.attach()`).
- **`src/commands/pool.ts`** — thin calls into `Pool` statics. No
  "claim" command — claiming is transparent via a plain `sandbox create`
  matching a configured pool.
- **`src/format.ts`** — logic pulled out for unit-testability without
  importing commander's `process.argv`-parsing module load: `parseTag`
  (throws commander's `InvalidArgumentError`, not a plain `Error`, for a
  clean stderr message), `formatSandboxList`, `formatImageList`.

## Testing

`node:test`, no added dep. `test/format.test.js` imports compiled
`dist/format.js` (built as its own banner-less `tsup` entry so it's
importable standalone); `npm test` runs `pretest` (build) first. Run:
`npm run test -w sandkiln-cli`.

## The bug that already happened here

**`program.command("sandbox")` already registers and attaches to
`program` — don't also call `program.addCommand(sandbox)` after.**
Crashed every subcommand at module-load time (`commander` throws on
duplicate registration, not just when the broken one runs).
`Command#command()` and `#addCommand()` are two different attachment
mechanisms — use one, not both, for the same command object.

## Building and verifying

```
npm run typecheck -w sandkiln-cli
npm run build -w sandkiln-cli
npm run test -w sandkiln-cli
```
Requires `sandkiln` (SDK) already built — a real workspace dependency
through `packages/sdk/dist/`. "Cannot find module 'sandkiln'" means build
the SDK first, not that this package is broken. Typecheck/build don't
prove a command works — verify live (`node packages/cli/dist/index.js
sandbox <subcommand>` against a real daemon); this is exactly how the
duplicate-command bug above was caught.

## Non-obvious things

- ESM bundle with a shebang banner (not ESM+CJS like the SDK — a binary
  doesn't need dual-format). Two `tsup` entries: `index.ts` (shebanged)
  and `format.ts` (banner-less, for test imports); `package.json`'s
  `bin` points only at `dist/index.js`.
- `cp` was deliberately kept as explicit `read`/`write` rather than a
  unified `sandbox:path` form — less magic parsing for a first version,
  not a final word against adding one later.
- **Published on npm as `sandkiln-cli`, not `kiln`** — `kiln` is already
  an unrelated third-party package (`node-kiln`); publishing under it
  gets a real `403`. The installed **command** is still `kiln` via
  `package.json`'s `bin.kiln` field. Install:
  `npm install -g sandkiln-cli`, run `kiln ...`.
- Published via `npm publish --provenance` in CI (`publish-cli.yml`,
  `workflow_dispatch` or a `cli-v*.*.*` tag) — builds `sandkiln` from
  source first, so a CLI release doesn't require a fresh SDK release.

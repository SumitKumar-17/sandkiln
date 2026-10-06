# AGENTS.md — kiln (CLI)

Read the root `AGENTS.md` first for project-wide conventions. This file
is scoped to this one package.

## What this package is

`kiln`: a thin `commander`-based CLI wrapping `packages/sdk` (the JS/TS
SDK) — for manual testing, agentic workflows, and debugging without
writing code. It should contain essentially no logic of its own beyond
argument parsing and formatting output; every actual operation delegates
straight to the SDK.

## Files

- `src/index.ts` — thin entry point only: builds the top-level `program`
  (name, description, the two global `--base-url`/`--token` options),
  calls each `commands/*.ts` module's `register*Commands(program)`, then
  `program.parseAsync(...)` with a backstop `.catch()`. No command logic
  of its own.
- `src/commands/shared.ts` — the handful of helpers genuinely shared
  across every resource group: `GlobalOptions`, `clientOptions`
  (resolves `--base-url`/`--token`, falling through to the SDK's own env
  var resolution when unset), `fail`/`handleApiError` (every action
  handler's error path: a clean `error: ...` message on stderr and exit
  code 1, never a raw stack trace).
- `src/commands/sandbox.ts` — the one large group:
  `sandbox create|get-or-create|by-name|ls|rm|exec|read|write|preview|
  pty|exec-stream|logs|snapshot|snapshots|resume|fork|chmod|chown|mkdir|
  rename|cp|symlink|readlink|truncate|ls-dir|mount|mounts|unmount`, each
  a thin call into `Sandbox`/`Sandbox.attach()`. Also owns the
  sandbox-specific option builders (`tagOption`/`envOption`/
  `buildEgressOption`) and two local helpers used only here:
  `attachSandbox` (reconstructs a `Sandbox` handle from just an id, no
  round-trip) and `followLogs` (the replay-then-live-tail logic shared by
  `exec-stream` and `logs`).

  `pty <id>` opens `Sandbox.pty()`'s `WebSocket` and pumps bytes between
  it and the local terminal: raw mode (`process.stdin.setRawMode(true)`)
  so every keystroke, including Ctrl+C, goes straight to the remote shell
  instead of Node intercepting it. A real live-tested gotcha here: the
  process used to hang for a full 10 seconds after the remote shell
  exited before this was traced to a guest-side bug (see
  `sandkiln-guest-agent`'s `pty.rs` and its own `AGENTS.md`) rather than
  anything in this file — the WebSocket's `close` event genuinely never
  fired until the guest actually tore down its side of the connection, so
  no amount of client-side cleanup here could have fixed it. Worth
  remembering if a similar hang ever shows up again: check whether the
  *server* side is actually closing the connection before assuming it's a
  local handle leak.

  `exec-stream <id> <command> [args...]` starts a background command
  (`Sandbox.execStream`) then immediately attaches and follows it
  (`followLogs`, shared with `logs`); `logs <id> [session-id]` lists
  sessions with no id, or attaches/follows an existing one with one —
  both print a replay-then-live-tail of the command's output and resolve
  the process's own exit code from the daemon's bracketed
  `[process exited with code N]` notice (parsed out of the plain-text
  notices `routes_logs.rs` sends alongside real output, not a separate
  structured message), which becomes this CLI invocation's own exit code.

  `resume`/`fork`/`get-or-create`/`by-name` call the SDK's static
  `Sandbox.resume`/`Sandbox.fork`/`Sandbox.getOrCreate`/`Sandbox.byName`
  directly (none acts on an already-existing handle — `resume`/`fork`
  take a snapshot id, `by-name`/`get-or-create` a name, not a sandbox id,
  so there's no existing handle to attach to). `sandbox create --image
  <id>` boots from a registered image instead of the daemon's default
  rootfs. `rm` defaults to the SDK's `stop()` persist-by-default behavior
  and reports whether the sandbox was preserved (with its snapshot id) or
  destroyed; `--destroy` opts into full destruction (`stop({ keep: false
  })`). `preview` is the one subcommand that makes no network call at all
  — it just prints `Sandbox.previewUrl()`'s pure result, same reasoning
  as why that SDK method itself does no round-trip; port-range validation
  lives in the SDK (`Sandbox.previewUrl` throws `RangeError`), not
  duplicated here, matching this package's own "essentially no logic of
  its own" rule.
- `src/commands/image.ts` — `image create|ls|rm`, each a thin call into
  `Image`'s static methods (an image has no instance handle at all, so
  there's no `Image.attach()` equivalent to `Sandbox.attach()`).
- `src/commands/drive.ts` — `drive create|ls|rm`, each a thin call into
  `Drive`'s static methods, same shape as `image.ts`.
- `src/commands/pool.ts` — `pool create|ls|rm`, each a thin call into
  `Pool`'s static methods. `pool create <id>` has no "claim" counterpart
  at all — claiming a warm instance is entirely transparent, done by a
  plain `sandbox create` whose `--image`/`--vcpu`/`--mem` match a
  configured pool (see `packages/sdk/AGENTS.md`'s `pool.ts` entry).

  Across all four `commands/*.ts` files: `--base-url`/`--token` are
  global options read via `clientOptions` (`commands/shared.ts`), which
  falls through to the SDK's own env var resolution when unset — don't
  reimplement that resolution in a command file, just pass `undefined`
  through. Every action handler catches its own errors via
  `handleApiError`/`fail`; `index.ts`'s `program.parseAsync(...)` has a
  `.catch()` backstop so nothing escapes as a raw stack trace.
- `src/format.ts` — the pure logic pulled out of `index.ts` specifically
  so it's unit-testable without importing the CLI's top-level commander
  wiring (which parses `process.argv` as a side effect of module load):
  `parseTag` (the `--tag key=value` parser — throws commander's
  `InvalidArgumentError`, not a plain `Error`, so a bad `--tag` value
  gets the same clean `error: ...` stderr message as everything else
  instead of an unhandled-exception stack trace), `formatSandboxList`
  (the `sandbox ls` output formatter, including the empty-list case), and
  `formatImageList` (the `image ls` equivalent).

## Testing

`node:test` + `node:assert`, no added dependency — matches the project's
convention of pulling pure logic out of framework plumbing so it's
testable (see root `AGENTS.md`'s `auth::token_matches` precedent) rather
than skipping tests because the rest of the file needs a live daemon.

- `test/format.test.js` covers `src/format.ts`.
- Tests import compiled output (`dist/format.js`), not TS source directly
  — `format.ts` is built as its own `tsup` entry (no shebang banner)
  specifically so it can be imported standalone. `npm test` runs
  `pretest` (`npm run build`) first, so it's always testing current code.
- Run: `npm run test -w sandkiln-cli` (or `cd packages/cli && npm test`).

## The bug that already happened here — read before touching command registration

**`program.command("sandbox")` already registers and attaches the
command to `program` — don't also call `program.addCommand(sandbox)`
afterward.** This exact mistake ("cannot add command 'sandbox' as
already have command 'sandbox'") crashed every single subcommand on the
first live run, because `commander` throws at *module load time* when a
duplicate registration happens, not just when the specific broken
subcommand is invoked. If you're restructuring how subcommands are
built, be aware `Command#command()` and `Command#addCommand()` are two
different ways to attach a command — use one, not both, for the same
command object.

## Building and verifying

```
npm run typecheck -w sandkiln-cli
npm run build -w sandkiln-cli
npm run test -w sandkiln-cli
```
**Requires `sandkiln` (the SDK) to already be built** — it's a real
workspace dependency resolved through `packages/sdk/dist/`, not source.
If typecheck fails with "Cannot find module 'sandkiln'", build the SDK
first (`npm run build -w sandkiln`), don't assume this package is broken.

Typecheck/build alone don't prove a command works — every subcommand
here needs live verification against a real daemon (same port-forward
pattern as `packages/sdk/AGENTS.md` describes, but running
`node packages/cli/dist/index.js sandbox <subcommand>` directly instead
of a throwaway script). This is exactly how the duplicate-command bug
above was caught — it didn't show up in typecheck or build, only when
actually run.

## Non-obvious things specific to this package

- Ships as an ESM bundle with a shebang banner (`tsup.config.ts`'s
  `banner: { js: "#!/usr/bin/env node" }`), not ESM+CJS like the SDK — a
  CLI binary doesn't need dual-format support the way a library does.
  `tsup.config.ts` builds two entries: `index.ts` (the shebanged
  executable) and `format.ts` (no banner, built standalone purely so
  tests can import it) — the `bin` field in `package.json` only ever
  points at `dist/index.js`.
- `cp` (a single unified `sandbox:path`-style copy command, as originally
  sketched in `ROADMAP.md`) was deliberately simplified to explicit
  `read`/`write` subcommands instead — less magic path-prefix parsing for
  a first version. If you're tempted to add a unified `cp`, that's a
  legitimate improvement, just don't assume the roadmap's original
  wording is the final word on the exact command shape.
- **Published on npm as `sandkiln-cli`, not `kiln`.** The short name
  `kiln` is already taken by a completely unrelated, pre-existing package
  (`node-kiln`, "Provides Kiln API functionality," owned by a third party
  since before this project existed — confirmed via `npm view kiln
  repository`, points at `boneskull/node-kiln`, nothing to do with
  sandkiln; a publish attempt under that name gets a real `403 Forbidden`
  from npm). The installed **command** is still `kiln` — only the npm
  package name differs from the binary name, which npm's `bin` field
  supports directly (`package.json`'s `bin.kiln` points at
  `dist/index.js` regardless of what `name` says). Install with
  `npm install -g sandkiln-cli`, then just run `kiln ...`.
- Published with `npm publish --provenance` from CI
  (`.github/workflows/publish-cli.yml`), not manually — mirrors
  `publish-sdk.yml`'s trigger (`workflow_dispatch` or a `cli-v*.*.*` tag)
  and its `NPM_TOKEN` requirement. Builds `sandkiln` from source first
  (this package depends on it as a real workspace dependency, same
  ordering constraint `ci.yml` already documents), so a CLI release can
  be tagged independently of, and doesn't require, a fresh SDK release.

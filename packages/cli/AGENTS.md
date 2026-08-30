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

- `src/index.ts` — the CLI. `sandbox create|ls|rm|exec|read|write|preview`
  subcommands, each a thin call into `Sandbox`/`Sandbox.attach()`.
  `--base-url`/`--token` are global options that fall through to the
  SDK's own env var resolution when unset — don't reimplement that
  resolution here, just pass `undefined` through. Every action handler
  catches its own errors and reports them on stderr with a non-zero exit
  (`handleApiError`/`fail`); `program.parseAsync(...)` at the bottom has
  a `.catch()` backstop so nothing escapes as a raw stack trace. `preview`
  is the one subcommand that makes no network call at all — it just
  prints `Sandbox.previewUrl()`'s pure result, same reasoning as why that
  SDK method itself does no round-trip; port-range validation lives in
  the SDK (`Sandbox.previewUrl` throws `RangeError`), not duplicated here,
  matching this package's own "essentially no logic of its own" rule.
- `src/format.ts` — the pure logic pulled out of `index.ts` specifically
  so it's unit-testable without importing the CLI's top-level commander
  wiring (which parses `process.argv` as a side effect of module load):
  `parseTag` (the `--tag key=value` parser — throws commander's
  `InvalidArgumentError`, not a plain `Error`, so a bad `--tag` value
  gets the same clean `error: ...` stderr message as everything else
  instead of an unhandled-exception stack trace) and `formatSandboxList`
  (the `sandbox ls` output formatter, including the empty-list case).

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
- Run: `npm run test -w kiln` (or `cd packages/cli && npm test`).

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
npm run typecheck -w kiln
npm run build -w kiln
npm run test -w kiln
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

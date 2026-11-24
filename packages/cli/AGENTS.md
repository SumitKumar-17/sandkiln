# AGENTS.md: kiln (CLI)

Read root `AGENTS.md` first. `kiln` is a thin `commander` CLI over `packages/sdk`, for manual testing, agent workflows and
debugging. It has no logic beyond argument parsing and output formatting. Published on npm as `sandkiln-cli` (`kiln` is
taken by an unrelated package and gives a `403`); the command is still `kiln` via `package.json`'s `bin.kiln`.

## Files (`src/`)

- **`index.ts`**: entry point. Builds `program` (global `--base-url`/`--token`), calls each `register*Commands`, then
  `parseAsync` with a `.catch()` backstop.
- **`commands/shared.ts`**: `GlobalOptions`, `clientOptions` (falls through to the SDK's env resolution),
  `fail`/`handleApiError` (clean `error: ...` on stderr, exit 1, no stack traces).
- **`commands/sandbox.ts`**: the large group (create, get-or-create, by-name, ls, rm, exec, read, write, preview, pty,
  tunnel, exec-stream, logs, snapshot(s), resume, fork, filesystem ops, mount(s), unmount), each a thin SDK call. Owns
  `tagOption`, `envOption`, `buildEgressOption`, `attachSandbox` (handle from an id, no round trip) and `followLogs`.
  - `pty` uses raw stdin so Ctrl+C reaches the remote shell. A 10s hang after shell exit was once a guest-side bug, so
    check the guest agent first if it recurs.
  - `tunnel` runs in the foreground until Ctrl+C or SIGTERM, then closes the handle.
  - `exec-stream` and `logs` take the exit code from the daemon's `[process exited with code N]` line.
  - `rm` keeps a snapshot by default; `--destroy` is `stop({ keep: false })`. `preview` prints `previewUrl()` with no
    network call.
- **`commands/image.ts`, `drive.ts`, `pool.ts`, `status.ts`**: thin calls into the SDK statics (`status` backs `kiln status`).
- **`format.ts`**: pure logic kept importable without commander's argv parsing: `parseTag` (throws
  `InvalidArgumentError`), `formatSandboxList`, `formatImageList`.

## Invariants

- `program.command("sandbox")` already attaches to `program`; do not also call `program.addCommand(sandbox)`. Commander throws at
  module load on duplicate registration and every subcommand crashes.
- ESM bundle with a shebang banner and two `tsup` entries: `index.ts` (shebang) and `format.ts` (banner-less, for tests).
  `bin` points only at `dist/index.js`.
- `cp` is explicit `read`/`write`, not a `sandbox:path` form.

## Testing and release

```
npm run typecheck -w sandkiln-cli && npm run build -w sandkiln-cli && npm run test -w sandkiln-cli
```

Needs `sandkiln` built first ("Cannot find module 'sandkiln'" means build the SDK). `test/format.test.js` imports
`dist/format.js`. Build success does not prove a command works: run `node packages/cli/dist/index.js sandbox <subcommand>`
against a real daemon. CI publishes with `npm publish --provenance` (`publish-cli.yml`, dispatch or a `cli-v*.*.*` tag),
building the SDK from source first.

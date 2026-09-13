# AGENTS.md — examples

Read the root `AGENTS.md` first for project-wide conventions. This file
is scoped to this one directory.

## What this is

Real, runnable reference projects built on the published SDKs (`sandkiln`
on npm and `sandkiln` on PyPI) — not toy snippets pasted into a README.
Each subdirectory is a standalone project with its own dependency
manifest; none of these are npm/pip workspace members of the packages
they demonstrate, since they're meant to look exactly like what an
external user of the published packages would write.

## Contents

- `code-playground/` — JS/TS, the `sandkiln` npm package. Reads code
  from a file or stdin, runs it inside a sandbox, prints
  stdout/stderr/exit code.
- `agent-runner/` — Python, the `sandkiln` PyPI package. Runs a
  hardcoded stand-in "agent-generated" script inside a sandbox and reads
  back a result file it produced.
- `dev-server-preview/` — JS/TS, the `sandkiln` npm package. Starts a
  server inside a sandbox and prints the URL to reach it from a browser
  via `Sandbox.previewUrl()` and the daemon's `/sandboxes/:id/preview/:port`
  reverse proxy.
- `interactive-terminal/` — JS/TS, the `sandkiln` npm package. Opens a
  live, bidirectional shell session inside a sandbox via `Sandbox.pty()`
  and wires it to this process's own terminal (raw mode, keystrokes
  passed straight through) — distinct from `code-playground`'s
  request/response `runCommand()`.
- `pool-warm-start/` — JS/TS, the `sandkiln` npm package. Configures a
  pre-warmed pool with `Pool.create()` and claims from it several times
  in a row (not just once), reporting each attempt as a clean claim or a
  health-check fallback rather than a single number — see its own
  README's "Why several attempts, not one" section before assuming a
  pool always produces a fast result; a single-attempt version of this
  example would be actively misleading given what real testing found.
- `exec-stream-logs/` — JS/TS. Starts a multi-second command detached
  inside a sandbox with `Sandbox.execStream()`, live-tails it via
  `.attachLogs()` (a `WebSocket`, same shape as `pty()`), then
  **reattaches to the same session after it has finished** and gets the
  identical log back in milliseconds — the distinctive part of the
  feature, since the daemon buffers a session's output independently of
  any connection. The one example here that deviates from the
  published-package rule below: it depends on the in-repo
  `packages/sdk` because these three methods aren't published yet, and
  its README says so prominently. Switch it back once a new npm version
  ships.
- `remote-storage-mount/` — JS/TS. Mounts an S3-compatible bucket into a
  sandbox at `/mnt/bucket`, writes and reads a file through it with
  ordinary `writeFile()`/`readFile()` calls, then unmounts and confirms
  the guest-side FUSE mount is really gone. Mounts have no SDK method
  yet, so the three `/sandboxes/:id/mounts` calls are raw `fetch()`
  against the daemon while the rest uses the published package — its
  README says so, and says why inventing SDK methods here would be the
  wrong call. Needs the optional FUSE kernel + rclone-injected rootfs
  setup from `SELF_HOSTING.md`, and an S3-compatible endpoint the user
  supplies via `S3_ENDPOINT`/`S3_ACCESS_KEY`/`S3_SECRET_KEY`/`S3_BUCKET`.

## Conventions

- Every example needs a real running `sandkilnd` to execute against —
  none of this is mocked. See root `SELF_HOSTING.md`.
- Genuinely minimal: no example here should grow into a framework. If an
  example needs its own abstraction layer to stay readable, that's a
  sign the abstraction belongs in a separate library, not this
  directory.
- Depend on the published `sandkiln` package (npm/PyPI), not on
  in-repo package source paths. The one allowed exception is an example
  for an SDK method that genuinely isn't published yet — point it at
  `packages/sdk`, say so visibly in its README, and switch it back on the
  next release (`exec-stream-logs/` is the current instance). An example
  for a daemon feature with no SDK surface at all calls the HTTP API
  directly instead; don't invent SDK methods to demo one.
- Each example's `README.md` states what it does, exact commands to run
  it, the env vars that configure the daemon connection, and a pointer
  to `SELF_HOSTING.md`.

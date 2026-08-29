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

## Conventions

- Every example needs a real running `sandkilnd` to execute against —
  none of this is mocked. See root `SELF_HOSTING.md`.
- Genuinely minimal: no example here should grow into a framework. If an
  example needs its own abstraction layer to stay readable, that's a
  sign the abstraction belongs in a separate library, not this
  directory.
- Depend on the published `sandkiln` package (npm/PyPI), not on
  in-repo package source paths.
- Each example's `README.md` states what it does, exact commands to run
  it, the env vars that configure the daemon connection, and a pointer
  to `SELF_HOSTING.md`.

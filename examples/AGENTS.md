# AGENTS.md — examples

Read root `AGENTS.md` first.

## What this is

Real, runnable reference projects on the published SDKs (`sandkiln` on
npm/PyPI) — not README snippets. Each subdirectory is standalone with its
own manifest, not a workspace member, so it looks like what an external
user would actually write.

## Contents

- `code-playground/` — JS/TS. Runs code from a file/stdin in a sandbox,
  prints stdout/stderr/exit code.
- `agent-runner/` — Python. Runs a stand-in "agent-generated" script,
  reads back a result file.
- `dev-server-preview/` — JS/TS. Starts a server in a sandbox, prints the
  browser URL via `Sandbox.previewUrl()` + the preview reverse proxy.
- `interactive-terminal/` — JS/TS. Live bidirectional shell via
  `Sandbox.pty()`, raw-mode passthrough to this process's terminal —
  distinct from `code-playground`'s request/response `runCommand()`.
- `snapshot-lifecycle/` — JS/TS. Snapshots a setup step, forks two
  independent branches from it, proves neither's writes leak into the
  other, then consumes the snapshot via `resume()` and confirms it's
  gone — the concrete fork-vs-resume distinction.
- `named-persistent-sandbox/` — JS/TS. `getOrCreate()` by name, writes a
  counter, stops with default (state-keeping) options, repeats as a
  simulated second process — proves the counter persists by name alone,
  no id tracked by the caller.
- `pool-warm-start/` — JS/TS. Claims from a pre-warmed pool several times
  in a row, reporting each as a clean claim or health-check fallback —
  see its README's "Why several attempts, not one" before assuming a pool
  claim is always instant-fast.
- `exec-stream-logs/` — JS/TS. Starts a detached command, live-tails via
  `.attachLogs()`, then **reattaches after it finished** and gets the
  identical buffered log back in milliseconds.
- `remote-storage-mount/` — JS/TS. Mounts an S3-compatible bucket at
  `/mnt/bucket`, read/write through it, unmounts, confirms the FUSE mount
  is gone. Needs the optional FUSE+rclone rootfs setup (`SELF_HOSTING.md`)
  and `S3_ENDPOINT`/`S3_ACCESS_KEY`/`S3_SECRET_KEY`/`S3_BUCKET`.
- `egress-policy/` — JS/TS. Pings a caller-supplied `EGRESS_EXAMPLE_TARGET_IP`
  under no-policy / `deny_all` / `deny_all + allowCidrs` — the real
  allow/deny behavior, not just "doesn't break normal use." No hardcoded
  target: a public IP silently "passes" on a host with no outbound route
  (a real dev-box finding), a LAN IP isn't portable.

## Conventions

- Every example needs a real running `sandkilnd` — nothing mocked. See
  root `SELF_HOSTING.md`.
- Stay minimal — if an example needs its own abstraction layer to stay
  readable, that abstraction belongs in a library, not here.
- Depend on the published `sandkiln` package, not in-repo source paths.
  Exception: an SDK method not yet published points at `packages/sdk`,
  stated visibly in its README, switched back next release
  (`exec-stream-logs/` is the current instance). A daemon feature with no
  SDK surface calls the HTTP API directly — don't invent SDK methods to demo it.
- Each `README.md` states what it does, exact run commands, config env
  vars, and a pointer to `SELF_HOSTING.md`.

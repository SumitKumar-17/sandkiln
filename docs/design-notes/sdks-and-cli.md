# Client SDKs and CLI

Both SDKs and the CLI talk to the daemon's HTTP API; logic is not
duplicated beyond what each language needs.

## JS/TS SDK (`sandkiln` on npm)

Matches the daemon's full surface. `Sandbox.create()` (tags, auth token,
`vcpuCount`/`memSizeMib`, `env`, `egress`, `rateLimit`, `drives`,
`imageId`), `list()` (tag-filterable), static `resume()`/`fork()`,
`runCommand()`, `readFile()`/`writeFile()`, `snapshot()`, `previewUrl()`,
`stop()`, `execStream()`/`listExecStreams()`/`attachLogs()`,
`mount()`/`listMounts()`/`unmount()`, `pty()`, `tunnel()`. ESM + CJS + types
via `tsup`. Published at npmjs.com/package/sandkiln (0.11.0, signed
provenance from CI). Verified against a live auth-enabled daemon, which is
how `stop()` returning 200 instead of the documented 204 was caught.

## Python SDK (`sandkiln` on PyPI)

Mirrors the JS SDK, including `resume()`/`fork()`/`snapshot()`/
`preview_url()` and resource overrides, with zero runtime dependencies
(stdlib `urllib`). Published via the OIDC trusted-publishing workflow
`.github/workflows/publish-python-sdk.yml`; see `packages/python/AGENTS.md`
for shipping the next version. Not in Python: `pty()`, `execStream()`.

Async client: `AsyncSandbox`/`AsyncDrive`/`AsyncImage`/`AsyncPool` mirror
the sync classes method for method on `asyncio.open_connection`
(`_http_async.py`), a minimal hand-rolled HTTP/1.1 client, the same shape
`sandkiln-vmm` uses for Firecracker's API and kept for the same
zero-dependency reason. They are separate classes, not one class with both
kinds of method: a mixed class fails as a silent event-loop stall, which is
worse than a wrong-class import (`packages/python/AGENTS.md`). Verified:
create/exec/read/write/list-dir with `env`, five concurrent `run_command()`
calls via `asyncio.gather`, snapshot to resume with `env` surviving.

## Filesystem operations

`chmod`/`chown`/`mkdir` (with `parents`)/`rename`/`copy`/`symlink`/
`readlink`/`truncate` and directory listing with metadata (name, is-dir,
is-symlink, size, permission bits, mtime), as vsock protocol commands next to
`exec`/`read_file`/`write_file`. This also wired up `list_dir`, which had
existed in the protocol and guest agent with no daemon route, SDK or CLI; its
shape was widened in place since nothing used it. SDK: `sandbox.chmod/chown/
mkdir/rename/copy/symlink/readlink/truncate/listDir`; CLI: `kiln sandbox
chmod|chown|mkdir|rename|cp|symlink|readlink|truncate|ls-dir`.

No path validation, same as `read_file`/`write_file`: the guest agent is a
dumb executor by design (its `AGENTS.md`), and a path resolves inside that
one microVM, not the host. Verified end to end against the real base rootfs
(21 integration checks: `mkdir -p` and its conflict case, chmod visible in a
later listing, rename and copy checked by content, symlink/readlink
round-trip, truncate byte-accurate, chown, and the error cases).

## Environment variables

`env: {key: value}` on `POST /sandboxes` is fixed for the sandbox's life;
`exec` and `exec-stream` accept their own `env`, merged over it with the
per-call value winning. Resolved daemon-side (`routes_exec::resolve_env`)
so the guest receives a single resolved `env` field on `Request::Exec` and
`ExecStreamHandshake`, with no notion of sandbox-level vs call-level. It
persists through snapshot, resume and fork like `tags` (no external resource
to re-apply, unlike `egress`). SDK: `Sandbox.create({env})`,
`runCommand(cmd, args, {env})`, `execStream(...)`; Python `env=` kwargs;
CLI `--env key=value` (repeatable). Verified in all three languages and the
CLI, including survival across snapshot to resume and fork.
`scripts/integration-tests/24-env-vars.sh`.

## Already covered

Interactive access exists as `kiln sandbox pty`. Converting an OCI image
into a rootfs is out of scope (see [ideas](ideas-not-started.md)).

## CLI (`kiln`)

A thin `commander` wrapper over the JS SDK, usable standalone for manual
testing, agent workflows and debugging.
- `kiln sandbox create|ls|rm|exec|exec-stream|logs|read|write|preview|
  snapshot|resume|fork|get-or-create|get|pty|tunnel|mount|mounts|unmount`
  plus `kiln image` and `kiln pool` and `kiln drive`. A single `cp` was
  replaced by explicit `read`/`write`: less magic than a `sandbox:path`
  prefix syntax for a first version.
- Published as `sandkiln-cli` (0.2.0, signed provenance). Not as `kiln`,
  which is an unrelated package owned by someone else; npm's `bin` field
  still installs the `kiln` command.

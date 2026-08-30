# sandkiln

Client SDK for [sandkiln](https://github.com/SumitKumar-17/sandkiln) — a
compute primitive for safely running untrusted or AI-generated code in
hardware-isolated Firecracker microVMs. Each sandbox is a real microVM:
its own kernel, its own filesystem, its own network.

This package is the client. It talks to a `sandkilnd` daemon over HTTP —
you need one running somewhere reachable (see the main repo for how to
run one; there is no hosted service). Zero runtime dependencies —
`urllib` from the standard library is all it needs.

## Install

```
pip install sandkiln
```

## Usage

```python
from sandkiln import Sandbox

sandbox = Sandbox.create(tags={"env": "ci"})

result = sandbox.run_command("python3", ["analyze.py"])
print(result.stdout, result.exit_code)

sandbox.write_file("/tmp/config.json", '{"ok": true}')
data = sandbox.read_file("/tmp/config.json")

running = Sandbox.list(tags={"env": "ci"})

sandbox.stop()
```

## Configuration

- **Daemon URL**: pass `base_url` to `Sandbox.create()`/`Sandbox.list()`,
  or set `SANDKILN_DAEMON_URL`. Defaults to `http://127.0.0.1:7777`.
- **Auth**: pass `auth_token`, or set `SANDKILN_AUTH_TOKEN`, if the
  daemon has `SANDKILN_AUTH_TOKEN` set. Omit entirely for an
  unauthenticated local daemon.

## API

- `Sandbox.create(tags=None, base_url=None, auth_token=None)` — boots a
  sandbox.
- `Sandbox.attach(id, base_url=None, auth_token=None)` — wraps an
  existing sandbox id without a network round-trip.
- `Sandbox.list(tags=None, base_url=None, auth_token=None)` — lists
  sandboxes; `tags` filters by exact match on every given key.
- `sandbox.run_command(command, args=None)` — returns an `ExecResult`
  (`stdout`, `stderr`, `exit_code`).
- `sandbox.read_file(path)` — returns file contents as `bytes`.
- `sandbox.write_file(path, content)` — `content` is `str` or `bytes`.
- `sandbox.stop()` — stops the sandbox and releases its resources.
- `sandbox.snapshot()` — saves the sandbox's full state to disk and stops
  it; returns a snapshot id.
- `Sandbox.resume(snapshot_id, base_url=None, auth_token=None)` — boots a
  new sandbox from a snapshot, **consuming** it (the snapshot is gone
  afterward).
- `Sandbox.fork(snapshot_id, base_url=None, auth_token=None)` — boots a
  new sandbox from a snapshot **without** consuming it, so it can be
  forked or resumed again later. Only one live fork of a given snapshot
  may run at a time — a second concurrent `fork()` raises
  `SandkilnApiError` with status 409 until the first is stopped; see
  `ROADMAP.md`'s "Persistence and snapshotting" section for why.

This mirrors the [JS/TS SDK](https://www.npmjs.com/package/sandkiln)
exactly — same daemon, same operations, Python-idiomatic naming
(`run_command` not `runCommand`, snake_case fields).

## Status

Early. This SDK matches the daemon's current HTTP API exactly — no more,
no less. Streamed command output and a drives API are planned; see the
[roadmap](https://github.com/SumitKumar-17/sandkiln/blob/main/ROADMAP.md).

## License

MIT

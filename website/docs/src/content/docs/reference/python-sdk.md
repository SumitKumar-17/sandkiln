---
title: Python SDK
description: Every Sandbox and Image method.
---

Not yet published to PyPI — install from the repository:

```bash
pip install ./packages/python
```

## Configuration

- **Daemon URL**: pass `base_url=` to any method, or set `SANDKILN_DAEMON_URL`. Defaults to `http://127.0.0.1:7777`.
- **Auth**: pass `auth_token=`, or set `SANDKILN_AUTH_TOKEN`. Omit entirely for an unauthenticated local daemon.

## `Sandbox`

- **`Sandbox.create(name=None, tags=None, base_url=None, auth_token=None, vcpu_count=None, mem_size_mib=None, image_id=None)`** — boots a sandbox. `name` is a caller-given identity, unique among live sandboxes and held snapshots (`409` if already taken) — see `by_name`/`get_or_create` below. `vcpu_count`/`mem_size_mib` override the daemon's configured defaults, subject to its ceiling. `image_id` boots from a registered image instead of the default rootfs.
- **`Sandbox.attach(id, base_url=None, auth_token=None)`** — wraps an existing sandbox id without a network round-trip.
- **`Sandbox.by_name(name, base_url=None, auth_token=None)`** — resolves a name to a *live* sandbox. Raises `SandkilnApiError` (`409`) if the name currently belongs to a stopped (snapshotted) sandbox instead — use `get_or_create` for that.
- **`Sandbox.get_or_create(name, tags=None, base_url=None, auth_token=None, vcpu_count=None, mem_size_mib=None)`** — resolves `name` to a sandbox in one race-safe call: live as-is, resumed if stopped, created fresh otherwise. Returns `(sandbox, created)`.
- **`Sandbox.list(tags=None, base_url=None, auth_token=None)`** — lists sandboxes; `tags` filters by exact match on every given key.
- **`sandbox.run_command(command, args=None)`** — returns an `ExecResult` (`stdout`, `stderr`, `exit_code`).
- **`sandbox.read_file(path)`** — returns file contents as `bytes`.
- **`sandbox.write_file(path, content)`** — `content` is `str` or `bytes`.
- **`sandbox.preview_url(port, path="/")`** — the URL a browser can open to reach a server listening on `port` inside the sandbox.
- **`sandbox.stop(keep=None)`** — stops the sandbox. Default (`keep` omitted or `True`) preserves state as a resumable snapshot, returns a `StopResult(kept, snapshot_id)`. `keep=False` fully destroys instead.
- **`sandbox.snapshot()`** — saves full state to disk and stops; returns a snapshot id.
- **`Sandbox.resume(snapshot_id, base_url=None, auth_token=None)`** — boots from a snapshot, **consuming** it.
- **`Sandbox.fork(snapshot_id, base_url=None, auth_token=None)`** — boots from a snapshot **without** consuming it. Raises `SandkilnApiError` (`409`) while an earlier fork is still live.
- **`Sandbox.list_snapshots(source_sandbox_id=None, base_url=None, auth_token=None)`** — lists snapshots. `source_sandbox_id` narrows to the one taken from that original sandbox id.

## `Image`

- **`Image.register(id, path, base_url=None, auth_token=None)`** — registers an already-built ext4 rootfs file at `path` on the daemon's own host filesystem, for `Sandbox.create(image_id=...)` to boot from. Not a file upload.
- **`Image.list(base_url=None, auth_token=None)`** / **`Image.delete(id, base_url=None, auth_token=None)`** — list registered images, or delete one (`409` while anything references it).

## Not yet in this SDK

Attaching drives at create time — the daemon and CLI don't expose it here either. See the project's Roadmap page.

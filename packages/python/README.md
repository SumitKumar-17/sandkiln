# sandkiln

Client SDK for [sandkiln](https://github.com/SumitKumar-17/sandkiln) — a
compute primitive for safely running untrusted or AI-generated code in
hardware-isolated Firecracker microVMs. Each sandbox is a real microVM:
its own kernel, its own filesystem, its own network.

This package is the client. It talks to a `sandkilnd` daemon over HTTP —
you need one running somewhere reachable (see the main repo for how to
run one; there is no hosted service). Zero runtime dependencies —
`urllib`/`asyncio` from the standard library is all it needs.

Every class below also ships as an `asyncio`-native mirror —
`AsyncSandbox`, `AsyncDrive`, `AsyncImage`, `AsyncPool` — with identical
method names and arguments, just `async def` and awaited. They're
deliberately separate classes rather than sync+async methods on one
class, since a sync call in an async context blocks the event loop
silently rather than raising a clear error.

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

- `Sandbox.create(name=None, tags=None, base_url=None, auth_token=None, vcpu_count=None, mem_size_mib=None, image_id=None, drives=None)` —
  boots a sandbox. `name` is a caller-given identity, unique among live
  sandboxes and held snapshots (409 if already taken) — see `by_name`/
  `get_or_create` below to find it again later. `vcpu_count`/
  `mem_size_mib` override the daemon's configured defaults for this one
  sandbox, subject to the daemon's configured ceiling. `image_id` boots
  from a registered image (see `Image.register` below) instead of the
  daemon's configured default rootfs. `drives` is a list of
  `DriveAttachment` to attach existing persistent drives (see `Drive`
  below). (A few more keyword arguments exist for rate limiting,
  per-sandbox environment variables, and egress policy — see the full
  reference in the main repo's docs for all of them.)
- `Sandbox.attach(id, base_url=None, auth_token=None)` — wraps an
  existing sandbox id without a network round-trip.
- `Sandbox.by_name(name, base_url=None, auth_token=None)` — resolves a
  name to a *live* sandbox and returns a handle to it. Raises
  `SandkilnApiError` (409) if the name currently belongs to a stopped
  (snapshotted) sandbox instead — use `get_or_create` if you want that
  resumed automatically.
- `Sandbox.get_or_create(name, tags=None, base_url=None, auth_token=None, vcpu_count=None, mem_size_mib=None)`
  — resolves `name` to a sandbox in one race-safe call: a live sandbox
  with this name is returned as-is, a stopped one is resumed, otherwise a
  fresh one is created and given this name. Returns `(sandbox, created)`.
- `Sandbox.list(tags=None, base_url=None, auth_token=None)` — lists
  sandboxes; `tags` filters by exact match on every given key.
- `sandbox.run_command(command, args=None)` — returns an `ExecResult`
  (`stdout`, `stderr`, `exit_code`).
- `sandbox.read_file(path)` — returns file contents as `bytes`.
- `sandbox.write_file(path, content)` — `content` is `str` or `bytes`.
- `sandbox.chmod(path, mode)`, `sandbox.chown(path, uid, gid)`,
  `sandbox.mkdir(path, parents=False)`, `sandbox.rename(from_path, to_path)`,
  `sandbox.copy(from_path, to_path)`, `sandbox.symlink(target, link_path)`,
  `sandbox.readlink(path)`, `sandbox.truncate(path, size)`,
  `sandbox.list_dir(path)` (returns `list[DirEntry]` with real
  `is_dir`/`is_symlink`/`size`/`mode`/`mtime` metadata) — the full
  filesystem operation set.
- `sandbox.preview_url(port, path="/")` — the URL a browser can open
  directly to reach a server listening on `port` inside this sandbox,
  proxied through the daemon.
- `sandbox.tunnel(guest_port, local_port, local_host="127.0.0.1")` — the
  reverse of `preview_url`: exposes a service running on **this
  machine** to code inside the sandbox. Returns a `TunnelHandle` with
  `.tunnel_id`/`.close()`. This package's only WebSocket-based feature —
  hand-rolled (RFC 6455 handshake and frame masking), since the standard
  library has no WebSocket client.
- `sandbox.mount(bucket, endpoint, access_key, secret_key, mount_path, read_only=False)`
  — mounts an S3-compatible bucket into the sandbox via `rclone mount`;
  ordinary `read_file()`/`write_file()` calls against `mount_path` then
  read/write through to the bucket. Needs the optional FUSE-capable
  rootfs setup described in `SELF_HOSTING.md`.
- `sandbox.list_mounts()` — lists every remote-storage mount currently
  active in the sandbox.
- `sandbox.unmount(mount_id)` — tears down one mount.
- `sandbox.stop(keep=None)` — stops the sandbox. By default (`keep`
  omitted or `True`) this *preserves* its state as a resumable snapshot,
  same as `snapshot()`, and returns a `StopResult(kept, snapshot_id)`.
  Pass `keep=False` for the old "just destroy it" behavior — no
  snapshot, nothing left to resume.
- `sandbox.snapshot()` — saves the sandbox's full state to disk and stops
  it; returns a snapshot id. The daemon can also do this on its own, for
  an idle sandbox, if the operator has `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`
  configured — see `Sandbox.list_snapshots` below for how to notice it and
  find the resulting snapshot.
- `Sandbox.resume(snapshot_id, base_url=None, auth_token=None)` — boots a
  new sandbox from a snapshot, **consuming** it (the snapshot is gone
  afterward).
- `Sandbox.fork(snapshot_id, base_url=None, auth_token=None)` — boots a
  new sandbox from a snapshot **without** consuming it, so it can be
  forked or resumed again later. Only one live fork of a given snapshot
  may run at a time — a second concurrent `fork()` raises
  `SandkilnApiError` with status 409 until the first is stopped; see
  `ROADMAP.md`'s "Persistence and snapshotting" section for why.
- `Sandbox.list_snapshots(source_sandbox_id=None, base_url=None, auth_token=None)`
  — lists snapshots. `source_sandbox_id` narrows this to the (at most one)
  snapshot taken from that original sandbox id — the way to find out
  whether a sandbox id that dropped out of `Sandbox.list()` turned into a
  snapshot (via a manual `snapshot()` or the daemon's auto-suspend) and
  what its new id is.
- `Image.register(id, path, base_url=None, auth_token=None)` — registers
  an already-built ext4 rootfs file at `path` on the daemon's own host
  filesystem under `id`, for `Sandbox.create(image_id=...)` to boot from.
  Not a file upload — the daemon can't verify the guest agent is baked
  in without root access to loop-mount it (`ImageInfo.guest_agent_verified`
  is always `False`); run `scripts/preflight-check.sh --root-checks
  --rootfs-image <path>` out of band first.
- `Image.list(base_url=None, auth_token=None)` / `Image.delete(id, base_url=None, auth_token=None)`
  — list registered images, or delete one (refused with 409 while any
  live sandbox, in-flight boot, or held snapshot still references it).
- `Drive.create(size_mib, base_url=None, auth_token=None)` — creates a
  new empty persistent drive, returns its id.
- `Drive.list(base_url=None, auth_token=None)` — lists drives, including
  every current holder and whether each is read-only.
- `Drive.delete(id, base_url=None, auth_token=None)` — permanently
  removes a drive and its backing file (refused with 409 while anything
  references it). Attach one via
  `Sandbox.create(drives=[DriveAttachment(id=..., read_only=False)])` —
  a drive attached read-only may be shared by many sandboxes at once,
  read-write needs exclusive access.
- `Pool.create(id, image_id=None, vcpu_count=None, mem_size_mib=None, warm_count=0, max_count=None, base_url=None, auth_token=None)`
  — configures a pre-warmed pool under a caller-given `id` (409 if
  already taken). `warm_count` is how many resumable snapshots to keep
  ready; `max_count` caps total live instances of this pool's profile
  (omitted means unbounded) — a create at the ceiling with nothing warm
  queues for up to 30s instead of exceeding it or rejecting outright.
- `Pool.list(base_url=None, auth_token=None)` — lists configured pools,
  including `warm_ready` (how many are actually ready right now).
- `Pool.delete(id, base_url=None, auth_token=None)` — removes a pool's
  configuration and destroys whatever it currently has warm. There's no
  `Pool.claim()` — claiming is entirely transparent: a plain
  `Sandbox.create()` matching a configured pool's image/resources
  resumes a warm instance instead of cold-booting.

This mirrors the [JS/TS SDK](https://www.npmjs.com/package/sandkiln)
closely — same daemon, same operations, Python-idiomatic naming
(`run_command` not `runCommand`, snake_case fields).

## Status

Published, and matches the daemon's current HTTP API closely, not
completely. Two daemon features have no wrapper here yet, available
only via the [JS/TS SDK](https://www.npmjs.com/package/sandkiln) or the
`kiln` CLI: streamed background exec (following a long-running
command's live output) and interactive `pty` sessions. See the
[roadmap](https://github.com/SumitKumar-17/sandkiln/blob/main/ROADMAP.md)
for what's next.

## License

MIT

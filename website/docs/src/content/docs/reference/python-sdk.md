---
title: Python SDK
description: Every Sandbox and Image method, with runnable examples.
---

Not yet published to PyPI — install from the repository:

```bash
pip install ./packages/python
```

## Configuration

- **Daemon URL**: pass `base_url=` to any method, or set `SANDKILN_DAEMON_URL`. Defaults to `http://127.0.0.1:7777`.
- **Auth**: pass `auth_token=`, or set `SANDKILN_AUTH_TOKEN`. Omit entirely for an unauthenticated local daemon.

```python
import os
from sandkiln import Sandbox

# Explicit per-call config...
sandbox = Sandbox.create(base_url="https://daemon.internal:7777", auth_token=os.environ["SANDKILN_TOKEN"])

# ...or set both as environment variables once and omit them everywhere:
# SANDKILN_DAEMON_URL=https://daemon.internal:7777
# SANDKILN_AUTH_TOKEN=...
same_sandbox = Sandbox.create()
```

## `Sandbox`

### Create, run, clean up — the basic loop

```python
from sandkiln import Sandbox, SandkilnApiError

sandbox = Sandbox.create(tags={"env": "ci", "owner": "pipeline"}, vcpu_count=2, mem_size_mib=1024)

result = sandbox.run_command("python3", ["analyze.py", "--input", "data.csv"])
if result.exit_code != 0:
    raise RuntimeError(f"analyze.py failed: {result.stderr}")
print(result.stdout)

sandbox.write_file("/tmp/report.json", '{"ok": true}')
report = sandbox.read_file("/tmp/report.json")

# One-shot run, never needed again -- skip the snapshot entirely.
sandbox.stop(keep=False)
```

- **`Sandbox.create(name=None, tags=None, base_url=None, auth_token=None, vcpu_count=None, mem_size_mib=None, image_id=None, rate_limit=None, drives=None)`** — boots a sandbox. `name` is a caller-given identity, unique among live sandboxes and held snapshots (`409` if already taken) — see `by_name`/`get_or_create` below. `vcpu_count`/`mem_size_mib` override the daemon's configured defaults, subject to its ceiling. `image_id` boots from a registered image instead of the default rootfs. `rate_limit` caps host I/O via Firecracker's own token-bucket limiter (unlimited if omitted). `drives` is a list of `DriveAttachment` to attach existing persistent drives (see `Drive` below). A call with no `drives`/`rate_limit` matching a configured pool's image/resources transparently resumes a warm snapshot instead of cold-booting — see `Pool` below.
- **`sandbox.run_command(command, args=None)`** — returns an `ExecResult` (`stdout`, `stderr`, `exit_code`).
- **`sandbox.read_file(path)`** — returns file contents as `bytes`.
- **`sandbox.write_file(path, content)`** — `content` is `str` or `bytes`.
- **`sandbox.chmod(path, mode)`**, **`sandbox.chown(path, uid, gid)`**, **`sandbox.mkdir(path, parents=False)`**, **`sandbox.rename(from_path, to_path)`**, **`sandbox.copy(from_path, to_path)`**, **`sandbox.symlink(target, link_path)`**, **`sandbox.readlink(path)`**, **`sandbox.truncate(path, size)`**, **`sandbox.list_dir(path)`** (returns `list[DirEntry]` with real `is_dir`/`is_symlink`/`size`/`mode`/`mtime` metadata) — the full filesystem operation set.
- **`sandbox.stop(keep=None)`** — stops the sandbox. Default (`keep` omitted or `True`) preserves state as a resumable snapshot, returns a `StopResult(kept, snapshot_id)`. `keep=False` fully destroys instead.

### Attaching to an id you already have

```python
import os
from sandkiln import Sandbox

# A worker process that only has an id passed to it -- no create() round-trip needed.
sandbox = Sandbox.attach(os.environ["SANDBOX_ID"], auth_token=os.environ["SANDKILN_TOKEN"])
sandbox.run_command("echo", ["still here"])
```

- **`Sandbox.attach(id, base_url=None, auth_token=None)`** — wraps an existing sandbox id without a network round-trip.

### Named sandboxes — find the same environment again later

```python
from sandkiln import Sandbox, SandkilnApiError

# First call today: creates a fresh sandbox named "agent-session-42".
# Every later call, any day: resumes it if it was stopped, returns it as-is if still live.
sandbox, created = Sandbox.get_or_create(name="agent-session-42")
if created:
    sandbox.run_command("pip", ["install", "-r", "requirements.txt"])  # one-time setup
sandbox.run_command("pytest")
sandbox.stop()  # preserved by default -- nothing to do here

# Elsewhere: resolve the same name to a *live* sandbox without creating anything.
try:
    live = Sandbox.by_name("agent-session-42")
except SandkilnApiError as e:
    pass  # 409 if the name currently belongs to a stopped sandbox -- use get_or_create for that
```

- **`Sandbox.by_name(name, base_url=None, auth_token=None)`** — resolves a name to a *live* sandbox. Raises `SandkilnApiError` (`409`) if the name currently belongs to a stopped (snapshotted) sandbox instead — use `get_or_create` for that.
- **`Sandbox.get_or_create(name, tags=None, base_url=None, auth_token=None, vcpu_count=None, mem_size_mib=None)`** — resolves `name` to a sandbox in one race-safe call: live as-is, resumed if stopped, created fresh otherwise. Returns `(sandbox, created)`.

### Listing and filtering

```python
ci_sandboxes = Sandbox.list(tags={"env": "ci"})
for info in ci_sandboxes:
    print(info.id, info.name, info.created_at)
```

- **`Sandbox.list(tags=None, base_url=None, auth_token=None)`** — lists sandboxes; `tags` filters by exact match on every given key.

### Dev-server preview

```python
sandbox = Sandbox.create()
sandbox.run_command("sh", ["-c", "npm run dev &"])  # background the dev server
url = sandbox.preview_url(3000, path="/health")
print(url)  # open this in a browser, or request it yourself
```

- **`sandbox.preview_url(port, path="/")`** — the URL a browser can open to reach a server listening on `port` inside the sandbox.

### Snapshot, resume, fork

```python
snapshot_id = sandbox.snapshot()  # save state, stop the VM

# ...later, possibly in a different process...
resumed = Sandbox.resume(snapshot_id)  # consumes the snapshot
resumed.run_command("echo", ["back from a snapshot"])

# Or keep the snapshot reusable by forking instead of resuming:
fork1 = Sandbox.fork(snapshot_id)
fork1.stop()  # frees the fork; the snapshot is still there
fork2 = Sandbox.fork(snapshot_id)  # fork it again
```

- **`sandbox.snapshot()`** — saves full state to disk and stops; returns a snapshot id.
- **`Sandbox.resume(snapshot_id, base_url=None, auth_token=None)`** — boots from a snapshot, **consuming** it.
- **`Sandbox.fork(snapshot_id, base_url=None, auth_token=None)`** — boots from a snapshot **without** consuming it. Raises `SandkilnApiError` (`409`) while an earlier fork is still live.
- **`Sandbox.list_snapshots(source_sandbox_id=None, base_url=None, auth_token=None)`** — lists snapshots. `source_sandbox_id` narrows to the one taken from that original sandbox id.

## `Image`

```python
from sandkiln import Image, Sandbox

registered = Image.register("node-lts-custom", "/home/t1000/images/node-lts-custom.ext4")
if not registered.guest_agent_verified:
    print(f"warning: {registered.verification_hint}")

sandbox = Sandbox.create(image_id="node-lts-custom")

images = Image.list()
Image.delete("node-lts-custom")  # 409 while any sandbox/snapshot still references it
```

- **`Image.register(id, path, base_url=None, auth_token=None)`** — registers an already-built ext4 rootfs file at `path` on the daemon's own host filesystem, for `Sandbox.create(image_id=...)` to boot from. Not a file upload.
- **`Image.list(base_url=None, auth_token=None)`** / **`Image.delete(id, base_url=None, auth_token=None)`** — list registered images, or delete one (`409` while anything references it).

## `Drive`

```python
from sandkiln import Drive, DriveAttachment, Sandbox

drive = Drive.create(512)  # 512 MiB, empty
sandbox = Sandbox.create(drives=[DriveAttachment(id=drive.id)])
# A second sandbox can share it read-only alongside others, but not read-write:
reader = Sandbox.create(drives=[DriveAttachment(id=drive.id, read_only=True)])

Drive.delete(drive.id)  # 409 while any sandbox or held snapshot still attaches it
```

- **`Drive.create(size_mib, base_url=None, auth_token=None)`** — creates a new empty persistent drive, returns its id.
- **`Drive.list(base_url=None, auth_token=None)`** — lists drives, including every current holder and whether each is read-only.
- **`Drive.delete(id, base_url=None, auth_token=None)`** — permanently removes a drive and its backing file.

## `Pool`

```python
from sandkiln import Pool, Sandbox

Pool.create("build-workers", vcpu_count=2, mem_size_mib=1024, warm_count=3)

# Some time later, once the background replenisher has warmed instances up:
# matches the pool's image/resources and no drives/rate_limit -- resumes
# a warm snapshot automatically instead of cold-booting. No special call.
sandbox = Sandbox.create(vcpu_count=2, mem_size_mib=1024)
```

- **`Pool.create(id, image_id=None, vcpu_count=None, mem_size_mib=None, warm_count=0, base_url=None, auth_token=None)`** — configures a pool under a caller-given `id` (`409` if already taken — delete it first to reconfigure). `warm_count` is how many resumable snapshots to keep ready.
- **`Pool.list(base_url=None, auth_token=None)`** — lists configured pools, including `warm_ready` (how many are actually ready right now — replenishment happens in the background, not instantly).
- **`Pool.delete(id, base_url=None, auth_token=None)`** — removes a pool's configuration and destroys whatever it currently has warm. A sandbox already claimed from it is unaffected.

There's no `Pool.claim()` — claiming is entirely transparent, done by `Sandbox.create()` itself matching a configured pool. See [Startup latency & the pre-warmed pool](../../architecture/startup-latency/) for real measured numbers, including a genuinely non-rare resume failure mode and how it's handled.

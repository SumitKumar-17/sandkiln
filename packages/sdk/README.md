# sandkiln

Client SDK for [sandkiln](https://github.com/SumitKumar-17/sandkiln) — a
compute primitive for safely running untrusted or AI-generated code in
hardware-isolated Firecracker microVMs. Each sandbox is a real microVM:
its own kernel, its own filesystem, its own network.

This package is the client. It talks to a `sandkilnd` daemon over HTTP —
you need one running somewhere reachable (see the main repo for how to
run one; there is no hosted service).

## Install

```
npm install sandkiln
```

## Usage

```ts
import { Sandbox } from "sandkiln";

const sandbox = await Sandbox.create({
  tags: { env: "ci" },
});

const result = await sandbox.runCommand("python3", ["analyze.py"]);
console.log(result.stdout, result.exitCode);

await sandbox.writeFile("/tmp/config.json", JSON.stringify({ ok: true }));
const bytes = await sandbox.readFile("/tmp/config.json");

const running = await Sandbox.list({ tags: { env: "ci" } });

await sandbox.stop();
```

`Sandbox` also has `chmod`/`chown`/`mkdir`/`rename`/`copy`/`symlink`/`readlink`/`truncate`/`listDir`
for the rest of the filesystem surface, `pty()` for an interactive shell,
`previewUrl()`/`tunnel()` for reaching a dev server either direction, and
`execStream()`/`attachLogs()`/`listExecStreams()` for a long-running
command's live output — see [API](#api) below. `Drive` and `Pool` are
separate namespaces for persistent drives and pre-warmed snapshot pools.

## Configuration

- **Daemon URL**: pass `baseUrl` to `Sandbox.create()`/`Sandbox.list()`,
  or set `SANDKILN_DAEMON_URL`. Defaults to `http://127.0.0.1:7777`.
- **Auth**: pass `authToken`, or set `SANDKILN_AUTH_TOKEN`, if the daemon
  has `SANDKILN_AUTH_TOKEN` set. Omit entirely for an unauthenticated
  local daemon.

## API

- `Sandbox.create(options?)` — boots a sandbox. `options.name` is a
  caller-given identity, unique among live sandboxes and held snapshots
  (409 if already taken) — see `byName`/`getOrCreate` below to find it
  again later. `options.tags`, `options.baseUrl`, `options.authToken`,
  `options.vcpuCount`, `options.memSizeMib` (both override the daemon's
  configured defaults for this one sandbox, subject to the daemon's
  configured ceiling), `options.imageId` (boots from a registered image
  — see `Image.register` below — instead of the daemon's configured
  default rootfs).
- `Sandbox.attach(id, options?)` — wraps an already-existing sandbox id
  without a network round-trip.
- `Sandbox.byName(name, options?)` — resolves a name to a *live* sandbox
  and returns a handle to it. Rejects (409) if the name currently
  belongs to a stopped (snapshotted) sandbox instead — use
  `getOrCreate` if you want that resumed automatically.
- `Sandbox.getOrCreate(options)` — resolves `options.name` to a sandbox
  in one race-safe call: a live sandbox with this name is returned
  as-is, a stopped one is resumed, otherwise a fresh one is created and
  given this name. Returns `{ sandbox, created }`.
- `Sandbox.list(options?)` — lists sandboxes. `options.tags` filters by
  exact match on every given key.
- `sandbox.runCommand(command, args?)` — runs a command, returns
  `{ stdout, stderr, exitCode }`.
- `sandbox.readFile(path)` — returns file contents as `Uint8Array`.
- `sandbox.writeFile(path, content)` — `content` is a `string` or
  `Uint8Array`.
- `sandbox.chmod(path, mode)`, `sandbox.chown(path, uid, gid)`,
  `sandbox.mkdir(path, options?)` (`{ parents?: true }`),
  `sandbox.rename(from, to)`, `sandbox.copy(from, to)`,
  `sandbox.symlink(target, linkPath)`, `sandbox.readlink(path)`,
  `sandbox.truncate(path, size)`, `sandbox.listDir(path)` (returns
  entries with real `isDir`/`isSymlink`/`size`/`mode`/`mtime`
  metadata) — the full filesystem operation set, over the same vsock
  channel `runCommand` uses.
- `sandbox.pty(options?)` — opens a live, interactive shell session and
  returns a native `WebSocket` (Node.js ≥22 or a browser, no added
  dependency). `options.cols`/`options.rows` size the terminal once, at
  open time.
- `sandbox.previewUrl(port, options?)` — the URL a browser can open
  directly to reach a server listening on `port` inside this sandbox,
  proxied through the daemon.
- `sandbox.tunnel(guestPort, options)` — the reverse of `previewUrl`:
  exposes a service running on **this machine** to code inside the
  sandbox. `options.localPort` (required), `options.localHost` (default
  `127.0.0.1`). Node-only. Returns a `TunnelHandle` with `.tunnelId` and
  `.close()`.
- `sandbox.execStream(command, args?)` — starts a command in the
  background, returns a session id immediately, for a long-running
  command you want to watch live instead of waiting on with
  `runCommand`.
- `sandbox.attachLogs(sessionId)` — returns a `WebSocket` that replays
  everything captured so far for that session, then live-tails new
  output — callable any number of times, including after the process
  has finished.
- `sandbox.listExecStreams()` — lists every streamed session tracked
  against this sandbox, running or finished.
- `sandbox.mount(options)` — mounts an S3-compatible bucket at
  `options.mountPath` via `rclone mount`; ordinary `readFile()`/
  `writeFile()` calls against that path then read/write through to the
  bucket. Needs the optional FUSE-capable rootfs setup described in
  `SELF_HOSTING.md`.
- `sandbox.listMounts()` — lists every remote-storage mount currently
  active in the sandbox.
- `sandbox.unmount(mountId)` — tears down one mount.
- `sandbox.stop(options?)` — stops the sandbox. By default
  (`options.keep` omitted or `true`) this *preserves* its state as a
  resumable snapshot, same as `snapshot()`, and returns
  `{ kept, snapshotId }`. Pass `{ keep: false }` for the old "just
  destroy it" behavior — no snapshot, nothing left to resume.
- `sandbox.snapshot()` — saves the sandbox's full state to disk and stops
  it; returns a snapshot id. The daemon can also do this on its own, for
  an idle sandbox, if the operator has `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`
  configured — see `Sandbox.listSnapshots` below for how to notice it and
  find the resulting snapshot.
- `Sandbox.resume(snapshotId, options?)` — boots a new sandbox from a
  snapshot, **consuming** it (the snapshot is gone afterward).
- `Sandbox.fork(snapshotId, options?)` — boots a new sandbox from a
  snapshot **without** consuming it, so it can be forked or resumed again
  later. Only one live fork of a given snapshot may run at a time — a
  second concurrent `fork()` rejects with a 409 until the first is
  stopped; see `ROADMAP.md`'s "Persistence and snapshotting" section for
  why.
- `Sandbox.listSnapshots(options?)` — lists snapshots.
  `options.sourceSandboxId` narrows this to the (at most one) snapshot
  taken from that original sandbox id — the way to find out whether a
  sandbox id that dropped out of `Sandbox.list()` turned into a snapshot
  (via a manual `snapshot()` or the daemon's auto-suspend) and what its
  new id is.
- `Image.register(id, path, options?)` — registers an already-built ext4
  rootfs file at `path` on the daemon's own host filesystem under `id`,
  for `Sandbox.create({ imageId })` to boot from. Not a file upload — the
  daemon can't verify the guest agent is baked in without root access to
  loop-mount it (`guestAgentVerified` is always `false`); run
  `scripts/preflight-check.sh --root-checks --rootfs-image <path>` out of
  band first.
- `Image.list(options?)` / `Image.delete(id, options?)` — list registered
  images, or delete one (refused with 409 while any live sandbox,
  in-flight boot, or held snapshot still references it).
- `Drive.create(sizeMib, options?)` — creates a new empty persistent
  drive, returns its id.
- `Drive.list(options?)` — lists drives, including every current holder
  (`attachedTo`) and whether each is read-only.
- `Drive.delete(id, options?)` — permanently removes a drive and its
  backing file (refused with 409 while anything references it). Attach
  one to a sandbox via `Sandbox.create({ drives: [{ id, readOnly? }] })`
  — a drive attached read-only may be shared by many sandboxes at once,
  read-write needs exclusive access.
- `Pool.create(id, options?)` — configures a pre-warmed pool under a
  caller-given `id` (409 if already taken). `options.imageId`/
  `options.vcpuCount`/`options.memSizeMib` define which creates can
  match it; `options.warmCount` (default `0`) is how many resumable
  snapshots to keep ready.
- `Pool.list(options?)` — lists configured pools, including `warmReady`
  (how many are actually ready right now).
- `Pool.delete(id, options?)` — removes a pool's configuration and
  destroys whatever it currently has warm. There's no `Pool.claim()` —
  claiming is entirely transparent: a plain `Sandbox.create()` matching
  a configured pool's image/resources resumes a warm instance instead of
  cold-booting.

## Status

Published and real. This SDK matches the daemon's current HTTP API
closely, not completely — snapshot lineage queries and time-travel
restore are supported by the daemon (and in some cases the CLI) but
have no wrapper here yet; see the
[roadmap](https://github.com/SumitKumar-17/sandkiln/blob/main/ROADMAP.md)
in the main repository for what's next. A [Python equivalent](https://pypi.org/project/sandkiln/)
mirrors this closely too, with its own small gap list (see that
package's README).

## License

MIT

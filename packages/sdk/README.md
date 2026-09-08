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
- `sandbox.previewUrl(port, options?)` — the URL a browser can open
  directly to reach a server listening on `port` inside this sandbox,
  proxied through the daemon.
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

## Status

Published and real. This SDK matches the daemon's current HTTP API
exactly — no more, no less. Streamed command output and attaching
persistent drives at create time (supported by the daemon and CLI, not
yet exposed here) are still open; see the [roadmap](https://github.com/SumitKumar-17/sandkiln/blob/main/ROADMAP.md)
in the main repository for what's next. A [Python equivalent](https://github.com/SumitKumar-17/sandkiln/tree/main/packages/python)
mirrors this exactly, but isn't published to PyPI yet.

## License

MIT

---
title: JS/TS SDK
description: Every Sandbox and Image method.
---

```bash
npm install sandkiln
```

## Configuration

- **Daemon URL**: pass `baseUrl` to any method, or set `SANDKILN_DAEMON_URL`. Defaults to `http://127.0.0.1:7777`.
- **Auth**: pass `authToken`, or set `SANDKILN_AUTH_TOKEN`. Omit entirely for an unauthenticated local daemon.

## `Sandbox`

- **`Sandbox.create(options?)`** — boots a sandbox. `options.name` is a caller-given identity, unique among live sandboxes and held snapshots (`409` if already taken) — see `byName`/`getOrCreate` below to find it again later. `options.tags`, `options.baseUrl`, `options.authToken`, `options.vcpuCount`, `options.memSizeMib` (override the daemon's configured defaults, subject to its ceiling), `options.imageId` (boots from a registered image instead of the default rootfs).
- **`Sandbox.attach(id, options?)`** — wraps an already-existing sandbox id without a network round-trip.
- **`Sandbox.byName(name, options?)`** — resolves a name to a *live* sandbox. Rejects (`409`) if the name currently belongs to a stopped (snapshotted) sandbox instead — use `getOrCreate` for that.
- **`Sandbox.getOrCreate(options)`** — resolves `options.name` to a sandbox in one race-safe call: live as-is, resumed if stopped, created fresh otherwise. Returns `{ sandbox, created }`.
- **`Sandbox.list(options?)`** — lists sandboxes. `options.tags` filters by exact match on every given key.
- **`sandbox.runCommand(command, args?)`** — returns `{ stdout, stderr, exitCode }`.
- **`sandbox.readFile(path)`** — returns file contents as `Uint8Array`.
- **`sandbox.writeFile(path, content)`** — `content` is a `string` or `Uint8Array`.
- **`sandbox.previewUrl(port, options?)`** — the URL a browser can open to reach a server listening on `port` inside the sandbox.
- **`sandbox.stop(options?)`** — stops the sandbox. Default (`options.keep` omitted or `true`) preserves state as a resumable snapshot, returns `{ kept, snapshotId }`. `{ keep: false }` fully destroys instead.
- **`sandbox.snapshot()`** — saves full state to disk and stops; returns a snapshot id.
- **`Sandbox.resume(snapshotId, options?)`** — boots from a snapshot, **consuming** it.
- **`Sandbox.fork(snapshotId, options?)`** — boots from a snapshot **without** consuming it. `409` while an earlier fork is still live.
- **`Sandbox.listSnapshots(options?)`** — lists snapshots. `options.sourceSandboxId` narrows to the one taken from that original sandbox id.

## `Image`

- **`Image.register(id, path, options?)`** — registers an already-built ext4 rootfs file at `path` on the daemon's own host filesystem, for `Sandbox.create({ imageId })` to boot from. Not a file upload. `guestAgentVerified` on the response is always `false` — see [Custom & managed images](/docs/concepts/images/).
- **`Image.list(options?)`** / **`Image.delete(id, options?)`** — list registered images, or delete one (`409` while anything references it).

## Not yet in this SDK

Attaching drives at create time — the daemon and CLI don't expose it here either. See the project's Roadmap page.

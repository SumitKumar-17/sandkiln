---
title: JS/TS SDK
description: Every Sandbox and Image method, with runnable examples.
---

```bash
npm install sandkiln
```

## Configuration

- **Daemon URL**: pass `baseUrl` to any method, or set `SANDKILN_DAEMON_URL`. Defaults to `http://127.0.0.1:7777`.
- **Auth**: pass `authToken`, or set `SANDKILN_AUTH_TOKEN`. Omit entirely for an unauthenticated local daemon.

```ts
import { Sandbox } from "sandkiln";

// Explicit per-call config...
const sandbox = await Sandbox.create({
  baseUrl: "https://daemon.internal:7777",
  authToken: process.env.SANDKILN_TOKEN,
});

// ...or set both as environment variables once and omit them everywhere:
// SANDKILN_DAEMON_URL=https://daemon.internal:7777
// SANDKILN_AUTH_TOKEN=...
const sameSandbox = await Sandbox.create();
```

## `Sandbox`

### Create, run, clean up — the basic loop

```ts
import { Sandbox } from "sandkiln";

const sandbox = await Sandbox.create({
  tags: { env: "ci", owner: "pipeline" },
  vcpuCount: 2,
  memSizeMib: 1024,
});

const result = await sandbox.runCommand("python3", ["analyze.py", "--input", "data.csv"]);
if (result.exitCode !== 0) {
  throw new Error(`analyze.py failed: ${result.stderr}`);
}
console.log(result.stdout);

await sandbox.writeFile("/tmp/report.json", JSON.stringify({ ok: true }));
const report = await sandbox.readFile("/tmp/report.json");

// One-shot run, never needed again — skip the snapshot entirely.
await sandbox.stop({ keep: false });
```

- **`Sandbox.create(options?)`** — boots a sandbox. `options.name` is a caller-given identity, unique among live sandboxes and held snapshots (`409` if already taken) — see `byName`/`getOrCreate` below to find it again later. `options.tags`, `options.baseUrl`, `options.authToken`, `options.vcpuCount`, `options.memSizeMib` (override the daemon's configured defaults, subject to its ceiling), `options.imageId` (boots from a registered image instead of the default rootfs).
- **`sandbox.runCommand(command, args?)`** — returns `{ stdout, stderr, exitCode }`.
- **`sandbox.readFile(path)`** — returns file contents as `Uint8Array`.
- **`sandbox.writeFile(path, content)`** — `content` is a `string` or `Uint8Array`.
- **`sandbox.stop(options?)`** — stops the sandbox. Default (`options.keep` omitted or `true`) preserves state as a resumable snapshot, returns `{ kept, snapshotId }`. `{ keep: false }` fully destroys instead.

### Attaching to an id you already have

```ts
// A worker process that only has an id passed to it — no create() round-trip needed.
const sandbox = Sandbox.attach(process.env.SANDBOX_ID!, { authToken: process.env.SANDKILN_TOKEN });
await sandbox.runCommand("echo", ["still here"]);
```

- **`Sandbox.attach(id, options?)`** — wraps an already-existing sandbox id without a network round-trip.

### Named sandboxes — find the same environment again later

```ts
// First call today: creates a fresh sandbox named "agent-session-42".
// Every later call, any day: resumes it if it was stopped, returns it as-is if still live.
const { sandbox, created } = await Sandbox.getOrCreate({ name: "agent-session-42" });
if (created) {
  await sandbox.runCommand("npm", ["install"]); // one-time setup for a brand-new environment
}
await sandbox.runCommand("npm", ["test"]);
await sandbox.stop(); // preserved by default — nothing to do here

// Elsewhere: resolve the same name to a *live* sandbox without creating anything.
try {
  const live = await Sandbox.byName("agent-session-42");
} catch (err) {
  // 409 if the name currently belongs to a stopped sandbox instead — use getOrCreate for that.
}
```

- **`Sandbox.byName(name, options?)`** — resolves a name to a *live* sandbox. Rejects (`409`) if the name currently belongs to a stopped (snapshotted) sandbox instead — use `getOrCreate` for that.
- **`Sandbox.getOrCreate(options)`** — resolves `options.name` to a sandbox in one race-safe call: live as-is, resumed if stopped, created fresh otherwise. Returns `{ sandbox, created }`.

### Listing and filtering

```ts
const ciSandboxes = await Sandbox.list({ tags: { env: "ci" } });
for (const info of ciSandboxes) {
  console.log(info.id, info.name, info.createdAt);
}
```

- **`Sandbox.list(options?)`** — lists sandboxes. `options.tags` filters by exact match on every given key.

### Dev-server preview

```ts
const sandbox = await Sandbox.create();
await sandbox.runCommand("sh", ["-c", "npm run dev &"]); // background the dev server
const url = sandbox.previewUrl(3000, { path: "/health" });
console.log(url); // open this in a browser, or fetch() it yourself
```

- **`sandbox.previewUrl(port, options?)`** — the URL a browser can open to reach a server listening on `port` inside the sandbox.

### Snapshot, resume, fork

```ts
const snapshotId = await sandbox.snapshot(); // save state, stop the VM

// ...later, possibly in a different process...
const resumed = await Sandbox.resume(snapshotId); // consumes the snapshot
await resumed.runCommand("echo", ["back from a snapshot"]);

// Or keep the snapshot reusable by forking instead of resuming:
const fork1 = await Sandbox.fork(snapshotId);
await fork1.stop(); // frees the fork; the snapshot is still there
const fork2 = await Sandbox.fork(snapshotId); // fork it again
```

- **`sandbox.snapshot()`** — saves full state to disk and stops; returns a snapshot id.
- **`Sandbox.resume(snapshotId, options?)`** — boots from a snapshot, **consuming** it.
- **`Sandbox.fork(snapshotId, options?)`** — boots from a snapshot **without** consuming it. `409` while an earlier fork is still live.
- **`Sandbox.listSnapshots(options?)`** — lists snapshots. `options.sourceSandboxId` narrows to the one taken from that original sandbox id.

## `Image`

```ts
import { Image, Sandbox } from "sandkiln";

const registered = await Image.register("node-lts-custom", "/home/t1000/images/node-lts-custom.ext4");
if (!registered.guestAgentVerified) {
  console.warn(registered.verificationHint);
}

const sandbox = await Sandbox.create({ imageId: "node-lts-custom" });

const images = await Image.list();
await Image.delete("node-lts-custom"); // 409 while any sandbox/snapshot still references it
```

- **`Image.register(id, path, options?)`** — registers an already-built ext4 rootfs file at `path` on the daemon's own host filesystem, for `Sandbox.create({ imageId })` to boot from. Not a file upload. `guestAgentVerified` on the response is always `false` — see [Custom & managed images](../../concepts/images/).
- **`Image.list(options?)`** / **`Image.delete(id, options?)`** — list registered images, or delete one (`409` while anything references it).

## Not yet in this SDK

Attaching drives at create time — the daemon and CLI don't expose it here either. See the project's Roadmap page.

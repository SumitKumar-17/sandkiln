---
title: Persist state across runs by name
description: Give a sandbox an identity you can find again tomorrow.
---

The default create/stop/gone lifecycle works for one-shot runs, but a lot of real workloads want the same environment back later — a dev environment, a long-lived agent session, anything worth not rebuilding from scratch every time.

## The pattern

```ts
import { Sandbox } from "sandkiln";

// First run: creates a fresh sandbox named "agent-session-42".
// Every later run: resumes it if it was stopped, or returns it as-is if still live.
const { sandbox, created } = await Sandbox.getOrCreate({ name: "agent-session-42" });

if (created) {
  // set up whatever this environment needs once
}

await sandbox.runCommand("your-agent-loop", []);
await sandbox.stop(); // preserved by default — nothing to do here
```

Next time this runs — tomorrow, next week — the exact same `getOrCreate({ name: "agent-session-42" })` call resumes the same filesystem state, no explicit snapshot id to track or pass around yourself.

## How it works underneath

`getOrCreate` resolves to one of three outcomes, race-safe under a per-name lock so two concurrent callers for a brand-new name can't both create a sandbox:

1. A live sandbox with this name → returned as-is.
2. A stopped (snapshotted) one → resumed.
3. Neither → created fresh, given this name.

See [Named sandboxes & persistent stop](../concepts/named-sandboxes/) for the full mechanics, and [Snapshots, resume, and fork](../concepts/snapshots/) for what "resumed" actually does to the sandbox's state.

## If you need it gone for good

`getOrCreate` never deletes anything — call `sandbox.stop({ keep: false })` explicitly (or `kiln sandbox rm --destroy`) when you're genuinely done with that identity and don't want the snapshot left behind.

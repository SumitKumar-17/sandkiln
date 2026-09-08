---
title: Auto-suspend idle sandboxes
description: Reclaim resources from forgotten sandboxes automatically.
---

A sandbox a caller forgot to stop otherwise sits there holding a VM, memory, and a network lease forever. Auto-suspend reclaims it automatically — without destroying its state.

## Enabling it

Set on the daemon, not per-sandbox:

```bash
SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS=1800 sandkilnd
```

Any sandbox with no exec/read/write activity for that many seconds is paused and snapshotted — the same pause+snapshot path a manual `snapshot()` call uses — freeing its VM/network resources while keeping it fully resumable.

## Finding what a vanished sandbox became

A sandbox reclaimed this way disappears from `GET /sandboxes` on its own, not just after an explicit stop. To find out what it turned into:

```ts
const snapshots = await Sandbox.listSnapshots({ sourceSandboxId: theOldId });
```

or `kiln sandbox snapshots --source <id>`. At most one snapshot can ever match, since a sandbox id is retired the moment it's snapshotted.

## Combining with a hard destroy backstop

`SANDKILN_IDLE_TIMEOUT_SECS` (plain destroy, no snapshot) can be set alongside auto-suspend as a backstop for a sandbox whose auto-suspend keeps failing for some reason — it must be configured strictly longer than the auto-suspend timeout, enforced at daemon startup. Auto-suspend always gets first crack at an idle sandbox; the destroy timeout only matters if that keeps not working.

## What doesn't get auto-suspended

A jailed sandbox, or one forked from a snapshot (shares its rootfs with the snapshot it came from) — both are structurally ineligible for the same reasons they can't be snapshotted manually either. See [Snapshots, resume, and fork](../../concepts/snapshots/).

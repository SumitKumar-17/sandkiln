---
title: Snapshots, resume, and fork
description: Save a sandbox's full state to disk and boot from it later.
---

A snapshot saves a running sandbox's full state — memory and disk — to disk, and stops the VM. The sandbox stops existing as a live sandbox; a snapshot record takes its place, ready to boot a new sandbox from without repeating boot or dependency installation.

## Taking a snapshot

- `POST /sandboxes/:id/snapshot` — pauses the VM, snapshots it, stops it. Returns a snapshot id.
- `DELETE /sandboxes/:id` (the default, `keep=true`) does the same thing internally, triggered by a stop rather than an explicit snapshot call. See [Sandbox lifecycle](../sandbox-lifecycle/).
- Auto-suspend does it automatically for an idle sandbox, if configured. See [Auto-suspend idle sandboxes](../../guides/auto-suspend/).

A jailed sandbox can't be snapshotted (`400`) — jailer support covers boot only, and Firecracker's own snapshot format bakes in the in-jail paths a resume can't reconstruct outside the jail.

## Resume — consumes the snapshot *record*, not the checkpoint

`POST /snapshots/:id/resume` boots a new sandbox from the snapshot, consuming the **record**: a second resume of the same id 404s, and the new sandbox owns a fresh rootfs and its network lease outright, exactly like a fresh `POST /sandboxes` create. By default it no longer deletes the underlying checkpoint data, though — see "Time-travel restore" below. `?retain_history=false` opts back into the original, fully-destructive behavior for a caller that wants zero retention overhead.

## Fork — doesn't consume it

`POST /snapshots/:id/fork` boots a new sandbox from the snapshot **without** consuming it, so the same prepared state can be resumed or forked again later. Only one live fork of a given snapshot may exist at a time — Firecracker has no verified mechanism to give two live descendants of one snapshot independent guest IP/MAC (frozen into the snapshotted state at the original boot), so a second `fork`/`resume` attempt while one is already live is rejected with `409`. This is **not** true simultaneous parallel forking; see [Snapshot, resume, and fork: the mechanism](../../internals/snapshot-resume-fork/) for the full reasoning, including a real rootfs-sharing corruption bug found and fixed there.

## Lineage — what a snapshot came from, and what came from it

Every snapshot records `parent_snapshot_id`: the snapshot its own source sandbox was itself resumed or forked from, `null` for a snapshot whose source sandbox was cold-booted (a lineage root). `GET /snapshots` returns it on every result, and a second filter, `?parent_snapshot_id=<id>`, answers the reverse question — unlike `?source_sandbox_id=` (at most one match, a sandbox id is retired the moment it's snapshotted), a parent can genuinely have more than one match over time, since a snapshot can be forked, that fork snapshotted and torn down, then forked again into a sibling line later. Together the two filters let you walk a full lineage tree in either direction, one request at a time.

## Time-travel restore — going back to an earlier checkpoint

Resuming used to mean "this exact save point is gone the moment you move forward from it." It no longer does: by default, `POST /snapshots/:id/resume` retires the checkpoint it consumes into `GET /snapshots/history` instead of deleting it, and `POST /snapshots/history/:id/restore` boots a new sandbox from any retired checkpoint — as many times as wanted, since restoring doesn't consume it either. `DELETE /snapshots/history/:id` reclaims one outright once you're done with it (there's no automatic expiry yet, and retention has a real disk cost — see [Snapshot, resume, and fork: the mechanism](../../internals/snapshot-resume-fork/) for what that actually looks like and why).

This is sequential, not branching: restoring an old checkpoint doesn't erase or invalidate whatever came after it in that lineage (those stay in history too), but a restore is refused (`409`, naming the current holder) while anything else sharing that checkpoint's frozen network identity is currently live or held. A restored sandbox owns its lease and rootfs outright, so — unlike a fork — it stays snapshottable afterward, letting you branch a new line of history from any point you've ever been.

## Finding a snapshot

`GET /snapshots` lists every held snapshot. `?source_sandbox_id=<id>` narrows it to the (at most one) snapshot taken from that original sandbox id — the way to go from "the sandbox id I had" to "the snapshot it became" after a manual snapshot, a default stop, or auto-suspend made it disappear from `GET /sandboxes`.

## Durability

Snapshot metadata is written atomically to disk alongside its state/memory files and reconciled back into the daemon at startup — a snapshot taken before a daemon crash or restart is still listable and resumable afterward. Not durable across a host *reboot* by default: snapshot storage lives under `$TMPDIR`, which may or may not be `tmpfs` on your host — see `SELF_HOSTING.md`'s persistent-state section.

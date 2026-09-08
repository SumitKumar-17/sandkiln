---
title: Sandbox lifecycle
description: Create, list, and stop — the core operations every client wraps.
---

A sandbox is a real Firecracker microVM: its own kernel, its own filesystem, its own network namespace. The daemon's lifecycle routes are the foundation everything else — snapshots, drives, images — builds on.

## Create

`POST /sandboxes` boots a new sandbox and returns its id. The request body is optional — every field has a sensible default:

- `name` — a caller-given identity, unique among live sandboxes and held snapshots. See [Named sandboxes](../named-sandboxes/).
- `tags` — key/value metadata, filterable at list time.
- `vcpu_count` / `mem_size_mib` — override the daemon's configured defaults for this one sandbox, rejected with `400` if `0` or above the daemon's configured ceiling.
- `image_id` — boot from a registered image instead of the daemon's default rootfs. See [Custom & managed images](../images/).
- `drives` — existing persistent drives to attach at boot. See [Drives](../drives/).

## List

`GET /sandboxes` lists every live sandbox. `?tag.<key>=<value>` filters by exact match on a tag — repeat the parameter for multiple tags, all must match. To resolve one specific name instead, use `GET /sandboxes/by-name/:name` (see [Named sandboxes](../named-sandboxes/)) rather than filtering the list.

## Exec, read, write

`POST /sandboxes/:id/exec` runs a command inside the sandbox and returns `stdout`/`stderr`/`exit_code` — a batch request/response, not a streaming session. `POST /sandboxes/:id/read-file` and `.../write-file` do the same for file contents, all three carried over the same vsock channel to the guest agent.

## Stop

`DELETE /sandboxes/:id` stops the sandbox. By default this *preserves* its state: the daemon pauses the VM, snapshots it to disk, and returns `200` with `{"kept": true, "snapshot_id": "..."}` — the same thing `POST /sandboxes/:id/snapshot` does, just triggered by stop. Pass `?keep=false` for the old "just destroy it" behavior — VM killed, network released, rootfs deleted, nothing left to resume, `204` with no body.

A sandbox that structurally can't be preserved (booted jailed — see [Privilege model](../../architecture/privilege-model/)) falls back to a full destroy automatically rather than erroring or leaking resources. A sandbox forked from a snapshot (see [Snapshots, resume, and fork](../snapshots/)) has nothing new to preserve either — its state already lives in the snapshot it came from — so it's silently destroyed on the default path too, reported as `{"kept": false}`.

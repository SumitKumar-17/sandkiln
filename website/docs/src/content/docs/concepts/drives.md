---
title: Drives
description: Persistent filesystem storage that outlives a single sandbox.
---

A drive is persistent block storage that outlives any one sandbox's lifetime — attach it, write to it, detach it, reattach it to a different sandbox later, and the data is still there.

## Creating and attaching

`POST /drives` with `{"size_mib": <n>}` creates a new empty drive and returns its id. Attach it to a sandbox at create time via `POST /sandboxes`'s `drives` field: `[{"id": "<drive-id>", "read_only": false}]`. Firecracker exposes it inside the guest as its own `virtio-blk` device — format and mount it like any other block device.

## Conflict detection

A drive attached read-write needs exclusive, single-holder access — attaching an already-attached (read-write) drive to a second sandbox is rejected with `409`. `GET /drives` reports every current holder (`attached_to`), each labeled `sandbox <id>` or `snapshot <id>` with its `read_only` flag, so you can see exactly what's blocking a conflicting attach.

## Read-only sharing

A drive attached read-only may be attached to arbitrarily many sandboxes (and held snapshots) at once — for data or a common base layer that doesn't need a per-sandbox copy. The rule: a new attach may coexist with what's already there only if every existing holder *and* the new attach are all read-only. A single read-write attachment — existing or requested — still needs exclusive access, exactly like before read-only sharing existed.

## Deleting

`DELETE /drives/:id` is refused (`409`) while anything still references it — a live sandbox, a held snapshot, or (per the rule above) any read-only holder at all. Detach or stop every holder first.

## What's not done yet

Neither SDK nor `kiln` exposes drives yet — the only way to create, attach, or delete a drive today is the raw HTTP API described above. See the project's Roadmap page for status.

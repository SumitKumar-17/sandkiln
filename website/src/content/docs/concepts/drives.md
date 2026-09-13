---
title: Drives
description: Persistent filesystem storage that outlives a single sandbox.
---

A drive is persistent block storage that outlives any one sandbox's lifetime — attach it, write to it, detach it, reattach it to a different sandbox later, and the data is still there.

## Creating and attaching

`POST /drives` with `{"size_mib": <n>}` creates a new empty drive and returns its id — or `Drive.create(sizeMib)`/`Drive.create(size_mib=...)` in the JS/TS and Python SDKs, or `kiln drive create <size-mib>`. Attach it to a sandbox at create time via `POST /sandboxes`'s `drives` field: `[{"id": "<drive-id>", "read_only": false}]` — the SDKs take a `drives`/`drives=` option on `Sandbox.create`/`create()` with the same shape (camelCase/snake_case per language), and `kiln sandbox create --drive <id[:ro]>` (repeatable). Firecracker exposes it inside the guest as its own `virtio-blk` device — format and mount it like any other block device.

## Conflict detection

A drive attached read-write needs exclusive, single-holder access — attaching an already-attached (read-write) drive to a second sandbox is rejected with `409`. `GET /drives` reports every current holder (`attached_to`), each labeled `sandbox <id>` or `snapshot <id>` with its `read_only` flag, so you can see exactly what's blocking a conflicting attach.

## Read-only sharing

A drive attached read-only may be attached to arbitrarily many sandboxes (and held snapshots) at once — for data or a common base layer that doesn't need a per-sandbox copy. The rule: a new attach may coexist with what's already there only if every existing holder *and* the new attach are all read-only. A single read-write attachment — existing or requested — still needs exclusive access, exactly like before read-only sharing existed.

## Deleting

`DELETE /drives/:id` is refused (`409`) while anything still references it — a live sandbox, a held snapshot, or (per the rule above) any read-only holder at all. Detach or stop every holder first.

## What's not done yet

A drive is local block storage — a `virtio-blk` device backed by a file on the daemon's own host, which the guest formats and mounts itself. If what you want is an S3-compatible bucket read and written as a directory inside the sandbox, that's a different feature: see [Remote storage mounts](../remote-storage/). Mounts have no exclusivity rules and aren't backed by host disk, but they need extra guest setup (a FUSE-capable kernel and `rclone` in the rootfs) and are daemon-API-only for now.

A drive can only be attached when a sandbox is created — there's no hot-attach to a running sandbox, and no explicit detach route either (detaching happens implicitly when the sandbox is stopped, which leaves the drive's backing file untouched). Drives also can't be resized after creation; `POST /drives`, `GET /drives`, and `DELETE /drives/:id` are the whole surface. See the project's Roadmap page for full status.

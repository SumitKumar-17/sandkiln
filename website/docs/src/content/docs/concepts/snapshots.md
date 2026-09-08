---
title: Snapshots, resume, and fork
description: Save a sandbox's full state to disk and boot from it later.
---

A snapshot saves a running sandbox's full state — memory and disk — to disk, and stops the VM. The sandbox stops existing as a live sandbox; a snapshot record takes its place, ready to boot a new sandbox from without repeating boot or dependency installation.

## Taking a snapshot

- `POST /sandboxes/:id/snapshot` — pauses the VM, snapshots it, stops it. Returns a snapshot id.
- `DELETE /sandboxes/:id` (the default, `keep=true`) does the same thing internally, triggered by a stop rather than an explicit snapshot call. See [Sandbox lifecycle](sandbox-lifecycle/).
- Auto-suspend does it automatically for an idle sandbox, if configured. See [Auto-suspend idle sandboxes](../guides/auto-suspend/).

A jailed sandbox can't be snapshotted (`400`) — jailer support covers boot only, and Firecracker's own snapshot format bakes in the in-jail paths a resume can't reconstruct outside the jail.

## Resume — consumes the snapshot

`POST /snapshots/:id/resume` boots a new sandbox from the snapshot, **consuming** it: the record and its on-disk files are gone afterward, and the new sandbox owns the rootfs and network lease outright, exactly like a fresh `POST /sandboxes` create. This is the original behavior, and stays the default for that reason — anything that already depends on "resume once, then it's gone" doesn't break.

## Fork — doesn't consume it

`POST /snapshots/:id/fork` boots a new sandbox from the snapshot **without** consuming it, so the same prepared state can be resumed or forked again later. Only one live fork of a given snapshot may exist at a time — Firecracker has no verified mechanism to give two live descendants of one snapshot independent rootfs backing files or independent guest IP/MAC (both are frozen into the snapshotted state at the original boot), so a second `fork`/`resume` attempt while one is already live is rejected with `409`. This is **not** true simultaneous parallel forking; see [Persistence model](../architecture/persistence-model/) for the full reasoning.

## Finding a snapshot

`GET /snapshots` lists every held snapshot. `?source_sandbox_id=<id>` narrows it to the (at most one) snapshot taken from that original sandbox id — the way to go from "the sandbox id I had" to "the snapshot it became" after a manual snapshot, a default stop, or auto-suspend made it disappear from `GET /sandboxes`.

## Durability

Snapshot metadata is written atomically to disk alongside its state/memory files and reconciled back into the daemon at startup — a snapshot taken before a daemon crash or restart is still listable and resumable afterward. Not durable across a host *reboot* by default: snapshot storage lives under `$TMPDIR`, which may or may not be `tmpfs` on your host — see `SELF_HOSTING.md`'s persistent-state section.

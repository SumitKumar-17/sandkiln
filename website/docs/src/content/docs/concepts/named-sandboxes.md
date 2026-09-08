---
title: Named sandboxes & persistent stop
description: Give a sandbox an identity you can find again by name.
---

By default, a sandbox is only reachable by an opaque id you have to remember or store yourself. Naming lets a sandbox carry a caller-given identity across its whole lifetime — including across a stop-then-resume cycle.

## Claiming a name

Pass `name` to `POST /sandboxes` — unique among live sandboxes and held snapshots at the moment it's claimed; a taken name is rejected with `409`. Naming is opt-in: omit it for an anonymous sandbox, same as before this existed. Names are restricted to `1–64` characters, `[A-Za-z0-9_-]` only.

## Resolving a name

`GET /sandboxes/by-name/:name` resolves a name to a *live* sandbox's id. It's deliberately narrow: if the name currently belongs to a stopped (snapshotted) sandbox instead, it returns `409` pointing at `get-or-create` below, rather than silently resuming it on your behalf — resuming a snapshot boots a VM, a real side effect a plain `GET` shouldn't perform implicitly.

## get-or-create — resume-or-create in one call

`POST /sandboxes/get-or-create` is the "give me a sandbox with this name, whatever state it's in" primitive:

- A live sandbox with this name → returned as-is (`created: false`).
- A stopped (snapshotted) one → resumed (`created: false`).
- Neither → a fresh sandbox is created and given this name (`created: true`).

Race-safe under a per-name lock: two concurrent `get-or-create` calls for the same brand-new name can't both create a sandbox — the second sees the first's result instead.

## Persistent-by-default stop

`DELETE /sandboxes/:id` now preserves state by default instead of destroying it — see [Sandbox lifecycle](sandbox-lifecycle/) for the full mechanics. Combined with naming, this is what makes "stop it, come back tomorrow, resume by name" a one-line operation instead of something you have to track ids for yourself: a name carries through `snapshot`/`resume`/`fork` when you re-specify it, so `get-or-create` after a stop resolves straight back to the same identity.

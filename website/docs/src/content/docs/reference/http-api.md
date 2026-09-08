---
title: Daemon HTTP API
description: Every route sandkilnd exposes.
---

Base URL defaults to `http://127.0.0.1:7777`. Every route below except `/healthz` and `/metrics` requires `Authorization: Bearer <token>` when `SANDKILN_AUTH_TOKEN` is set — see [Auth](/docs/concepts/auth/).

## Sandboxes

| Route | What it does |
|---|---|
| `POST /sandboxes` | Boot a sandbox. Body: `name?`, `tags?`, `vcpu_count?`, `mem_size_mib?`, `image_id?`, `drives?`. |
| `GET /sandboxes` | List sandboxes. `?tag.<key>=<value>` filters (repeatable, all must match). |
| `GET /sandboxes/by-name/:name` | Resolve a name to a *live* sandbox's id. `409` if the name currently belongs to a held snapshot instead. |
| `POST /sandboxes/get-or-create` | Resolve-or-create by name in one call. Body: `name` (required), `tags?`, `vcpu_count?`, `mem_size_mib?`. |
| `DELETE /sandboxes/:id` | Stop a sandbox. Preserves state as a snapshot by default (`200`, `{"kept": true, "snapshot_id": "..."}`); `?keep=false` fully destroys instead (`204`). See [Sandbox lifecycle](/docs/concepts/sandbox-lifecycle/). |
| `POST /sandboxes/:id/exec` | Run a command. Body: `{"command": "...", "args": [...]}`. Returns `stdout`/`stderr`/`exit_code`. |
| `POST /sandboxes/:id/read-file` | Body: `{"path": "..."}`. Returns `{"content_base64": "..."}`. |
| `POST /sandboxes/:id/write-file` | Body: `{"path": "...", "content_base64": "..."}`. |

## Snapshots

| Route | What it does |
|---|---|
| `POST /sandboxes/:id/snapshot` | Pause, snapshot, stop. Returns a snapshot id. |
| `GET /snapshots` | List snapshots. `?source_sandbox_id=<id>` narrows to the one taken from that sandbox. |
| `POST /snapshots/:id/resume` | Boot a new sandbox from the snapshot, **consuming** it. |
| `POST /snapshots/:id/fork` | Boot a new sandbox from the snapshot **without** consuming it. `409` while an earlier fork is still live. |
| `DELETE /snapshots/:id` | Delete a snapshot outright. `409` while a fork of it is still live. |

## Drives

| Route | What it does |
|---|---|
| `POST /drives` | Body: `{"size_mib": <n>}`. Creates an empty drive. |
| `GET /drives` | List drives, including current holders (`attached_to`) and their `read_only` flag. |
| `DELETE /drives/:id` | Delete a drive. `409` while anything still references it. |

## Images

| Route | What it does |
|---|---|
| `POST /images` | Body: `{"id": "...", "path": "..."}`. Registers an already-built ext4 rootfs from a host path. |
| `GET /images` | List registered images (`guest_agent_verified` is always `false`, see [Custom & managed images](/docs/concepts/images/)). |
| `DELETE /images/:id` | Delete an image. `409` while any live sandbox, in-flight boot, or held snapshot references it. |

## Preview

| Route | What it does |
|---|---|
| `GET/POST/... /sandboxes/:id/preview/:port[/*path]` | Reverse-proxies a full HTTP request to that port inside the sandbox. Accepts the token as `?token=` as well as a header. |

## Operational

| Route | What it does |
|---|---|
| `GET /healthz` | Unauthenticated. Returns `ok`. |
| `GET /metrics` | Unauthenticated. Prometheus text-exposition format. |

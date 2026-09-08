---
title: Daemon HTTP API
description: Every route sandkilnd exposes.
---

Base URL defaults to `http://127.0.0.1:7777`. Every route below except `/healthz` and `/metrics` requires `Authorization: Bearer <token>` when `SANDKILN_AUTH_TOKEN` is set — see [Auth](../../concepts/auth/).

## A complete session, in raw HTTP

```bash
TOKEN=...
BASE=http://127.0.0.1:7777

# Create, named so it can be found again by name later.
curl -s -X POST "$BASE/sandboxes" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"name":"build-worker","tags":{"env":"ci"},"vcpu_count":2,"mem_size_mib":1024}'
# {"id":"..."}

curl -s -X POST "$BASE/sandboxes/build-worker/exec" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"command":"npm","args":["test"]}'
# {"stdout":"...","stderr":"","exit_code":0}
```

Every id above is written as `build-worker` for readability — the real
`id` a create call actually returns is an opaque UUID; resolve a name
back to its live id first with `GET /sandboxes/by-name/build-worker`
if you only have the name (see [Named sandboxes](../../concepts/named-sandboxes/)).

```bash
# Stop it -- preserves state as a snapshot by default.
curl -s -X DELETE "$BASE/sandboxes/build-worker" -H "Authorization: Bearer $TOKEN"
# {"kept":true,"snapshot_id":"..."}

# Tomorrow: resolve the same name back to a live sandbox in one call,
# resuming the snapshot above automatically.
curl -s -X POST "$BASE/sandboxes/get-or-create" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"name":"build-worker"}'
# {"id":"...","created":false}
```

## Sandboxes

| Route | What it does |
|---|---|
| `POST /sandboxes` | Boot a sandbox. Body: `name?`, `tags?`, `vcpu_count?`, `mem_size_mib?`, `image_id?`, `drives?`. |
| `GET /sandboxes` | List sandboxes. `?tag.<key>=<value>` filters (repeatable, all must match). |
| `GET /sandboxes/by-name/:name` | Resolve a name to a *live* sandbox's id. `409` if the name currently belongs to a held snapshot instead. |
| `POST /sandboxes/get-or-create` | Resolve-or-create by name in one call. Body: `name` (required), `tags?`, `vcpu_count?`, `mem_size_mib?`. |
| `DELETE /sandboxes/:id` | Stop a sandbox. Preserves state as a snapshot by default (`200`, `{"kept": true, "snapshot_id": "..."}`); `?keep=false` fully destroys instead (`204`). See [Sandbox lifecycle](../../concepts/sandbox-lifecycle/). |
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
| `GET /images` | List registered images (`guest_agent_verified` is always `false`, see [Custom & managed images](../../concepts/images/)). |
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

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
| `POST /sandboxes` | Boot a sandbox. Body: `name?`, `tags?`, `vcpu_count?`, `mem_size_mib?`, `image_id?`, `drives?`, `rate_limit?` (`{bandwidth_bytes_per_sec?, ops_per_sec?}`, Firecracker's own token-bucket limiter). A request with no `drives`/`rate_limit` matching a configured pool's image/resources transparently resumes a warm snapshot instead of cold-booting — see [Startup latency & the pre-warmed pool](../../architecture/startup-latency/). |
| `GET /sandboxes` | List sandboxes. `?tag.<key>=<value>` filters (repeatable, all must match). |
| `GET /sandboxes/history` | Durable history of every sandbox that ever existed and how it ended (`sqlite`-backed, survives a daemon restart, unlike this list itself). `?live_only=true`, `?limit=<n>`. |
| `GET /sandboxes/by-name/:name` | Resolve a name to a *live* sandbox's id. `409` if the name currently belongs to a held snapshot instead. |
| `POST /sandboxes/get-or-create` | Resolve-or-create by name in one call. Body: `name` (required), `tags?`, `vcpu_count?`, `mem_size_mib?`, `image_id?`, `rate_limit?`, `drives?` (the last four used only if a fresh sandbox is created). |
| `DELETE /sandboxes/:id` | Stop a sandbox. Preserves state as a snapshot by default (`200`, `{"kept": true, "snapshot_id": "..."}`); `?keep=false` fully destroys instead (`204`). See [Sandbox lifecycle](../../concepts/sandbox-lifecycle/). |
| `POST /sandboxes/:id/exec` | Run a command. Body: `{"command": "...", "args": [...]}`. Returns `stdout`/`stderr`/`exit_code`. |
| `POST /sandboxes/:id/read-file` | Body: `{"path": "..."}`. Returns `{"content_base64": "..."}`. |
| `POST /sandboxes/:id/write-file` | Body: `{"path": "...", "content_base64": "..."}`. |
| `POST /sandboxes/:id/chmod` | Body: `{"path": "...", "mode": <octal-as-int>}`. |
| `POST /sandboxes/:id/chown` | Body: `{"path": "...", "uid": <n>, "gid": <n>}`. |
| `POST /sandboxes/:id/mkdir` | Body: `{"path": "...", "parents"?: <bool>}`. |
| `POST /sandboxes/:id/rename` | Body: `{"from": "...", "to": "..."}`. |
| `POST /sandboxes/:id/copy` | Body: `{"from": "...", "to": "..."}`. |
| `POST /sandboxes/:id/symlink` | Body: `{"target": "...", "link_path": "..."}`. |
| `POST /sandboxes/:id/readlink` | Body: `{"path": "..."}`. Returns `{"target": "..."}`. |
| `POST /sandboxes/:id/truncate` | Body: `{"path": "...", "size": <n>}`. |
| `POST /sandboxes/:id/list-dir` | Body: `{"path": "..."}`. Returns real per-entry metadata: type, permission bits, size, mtime. |
| `GET /sandboxes/:id/pty[?cols=&rows=]` | Upgrades to a WebSocket: a live, bidirectional shell session, distinct from `exec`'s request/response shape. Accepts the token as `?token=` as well as a header, since neither browsers' nor Node's native `WebSocket` constructor can set a custom header. A per-sandbox concurrent-session cap (64) is enforced. |

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

## Pools

Pool *configuration* only — replenishment happens in the background, and claiming is entirely transparent (a matching `POST /sandboxes`, above). See [Startup latency & the pre-warmed pool](../../architecture/startup-latency/).

| Route | What it does |
|---|---|
| `POST /pools` | Configure a pool. Body: `id` (required, caller-given, `409` if already taken), `image_id?`, `vcpu_count?`, `mem_size_mib?`, `warm_count` (required, `0` is valid — an inert pool), `max_count?` (a ceiling on total live instances of this pool's profile — omitted means unbounded, the original behavior). A `POST /sandboxes` matching a pool at its `max_count` ceiling with nothing warm queues (up to 30s) instead of exceeding it or rejecting outright — `503` if nothing frees up in time. |
| `GET /pools` | List configured pools, including `warm_ready` (how many resumable snapshots are actually ready right now) and `claimed` (how many live instances of this pool's profile exist right now, warm-resumed or cold-created alike). |
| `DELETE /pools/:id` | Remove a pool's configuration and destroy whatever it currently has warm. A sandbox already claimed from it is unaffected. |

## Preview

| Route | What it does |
|---|---|
| `GET/POST/... /sandboxes/:id/preview/:port[/*path]` | Reverse-proxies a full HTTP request to that port inside the sandbox. Accepts the token as `?token=` as well as a header. |

## Operational

| Route | What it does |
|---|---|
| `GET /healthz` | Unauthenticated. Returns `ok`. |
| `GET /metrics` | Unauthenticated. Prometheus text-exposition format. |

---
title: Share a drive read-only
description: Attach the same drive to many sandboxes at once.
---

A common pattern: one drive holds data or a base layer many sandboxes need to read, but none of them need to write. Read-only sharing exists for exactly this — no per-sandbox copies, no coordination needed.

## Seed it once

A drive read-only-shared later still needs writing at least once, before anything ever attaches it read-only — a read-only mount can't format or write to it.

```bash
curl -X POST http://127.0.0.1:7777/drives -d '{"size_mib": 512}'
# {"id": "<drive-id>", ...}

curl -X POST http://127.0.0.1:7777/sandboxes \
  -d '{"drives":[{"id":"<drive-id>"}]}'
# attach read-write, seed it, then stop this sandbox (or delete it with ?keep=false)
```

## Attach it read-only, as many times as you need

```bash
curl -X POST http://127.0.0.1:7777/sandboxes \
  -d '{"drives":[{"id":"<drive-id>","read_only":true}]}'
```

Repeat this against as many sandboxes as you want — every one succeeds, concurrently, as long as every existing holder and every new attach are all `read_only: true`. A single read-write attach — existing or newly requested — still needs exclusive access; see [Drives](../concepts/drives/) for the exact rule.

## Check who's holding it

```bash
curl http://127.0.0.1:7777/drives
```

`attached_to` lists every current holder (`sandbox <id>` or `snapshot <id>`) with its `read_only` flag — useful for confirming a conflict's actual cause, or checking a drive is free before deleting it.

---
title: Auth
description: Bearer-token authentication, and how every client resolves it.
---

The daemon supports a single shared bearer token, gating every `/sandboxes*`, `/drives*`, `/images*`, and `/snapshots*` route. `/healthz` and `/metrics` stay open regardless — they're operational data about the daemon, not sandbox data.

## Enabling it

Set `SANDKILN_AUTH_TOKEN` on the daemon. Unset (the default) means the API is **completely open** — fine for local dev, not for anything reachable beyond localhost. The daemon warns loudly at startup if it starts without one configured, so this is never a silent gap.

## Sending it

`Authorization: Bearer <token>` on every request. All three clients resolve the token the same way, so you only configure it once per environment:

- JS/TS: pass `authToken` to `Sandbox.create()`/`list()`/etc., or set `SANDKILN_AUTH_TOKEN` in the environment.
- Python: pass `auth_token=`, or set `SANDKILN_AUTH_TOKEN`.
- CLI: `--token`, or `SANDKILN_AUTH_TOKEN`.

## The one exception: preview URLs

`/sandboxes/:id/preview/:port` accepts the token as a `?token=` query parameter as well as the header — the thing hitting that URL is typically a browser tab or `<iframe>`, neither of which can set a custom header on a plain navigation. See [Dev-server preview](/docs/concepts/preview/) for why that's an accepted, deliberate trade-off scoped to exactly this one route.

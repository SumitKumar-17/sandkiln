---
title: Dev-server preview
description: Reach a server running inside a sandbox from a browser.
---

If a sandbox is running a dev server — or any HTTP server — on some port, the daemon's preview route proxies a real HTTP request to it over the bridge network, so you can open it in a browser without exposing the sandbox's network directly.

## Getting a preview URL

- JS/TS: `sandbox.previewUrl(port, { path })`
- Python: `sandbox.preview_url(port, path=)`
- CLI: `kiln sandbox preview <id> <port> [--path <path>]`

All three build the same thing: `GET/POST/... /sandboxes/:id/preview/:port[/path]`, pure and network-free like `attach` — nothing is created or awaited up front, the daemon proxies lazily on each request.

## Auth in a browser context

If the client has an auth token configured, it's appended as a `?token=` query parameter rather than sent as a header — the caller of this URL is typically a browser tab or an `<iframe src=...>` embed, neither of which can attach an `Authorization` header on a plain navigation. The daemon's preview route accepts the token either way. See [Auth](../auth/).

## What's not done yet

Plain HTTP request/response proxying only — WebSocket proxying (needed for dev-server HMR/live-reload) isn't implemented. An `Upgrade: websocket` request currently just gets its `Connection`/`Upgrade` headers stripped like any other hop-by-hop header, which won't upgrade correctly. This is a real, scoped-out follow-up, not silently broken.

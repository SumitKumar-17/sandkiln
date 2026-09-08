---
title: Introduction
description: What sandkiln is, how it's shaped, and where to go next.
---

sandkiln is a compute primitive for safely running untrusted or AI-generated code. Every sandbox is a real Firecracker microVM — its own kernel, its own filesystem, its own network namespace — not a container with extra steps. There's no hosted service: you run a `sandkilnd` daemon yourself, and talk to it from a client.

## The pieces

- **`sandkilnd`** — the daemon. An axum + tokio HTTP API that drives Firecracker directly: boot, snapshot/resume, networking, drives, images.
- **JS/TS SDK** (`sandkiln` on npm) — a thin, fully-typed client over the daemon's HTTP API.
- **Python SDK** (not yet on PyPI — install from the repo) — mirrors the JS SDK exactly.
- **CLI** (`sandkiln-cli` on npm, installs the `kiln` command) — every operation the SDKs expose, from the command line.

## Where to go from here

- New to sandkiln? Start with [Self-hosting quickstart](self-hosting/) to get a daemon running, then pick your language: [JS/TS](js/), [Python](python/), or the [CLI](cli/).
- Want to understand a specific capability before using it? See [Core Concepts](../concepts/sandbox-lifecycle/).
- Have a specific task in mind? See [Guides](../guides/run-untrusted-code/).
- Looking for exact method signatures or HTTP routes? See [Reference](../reference/http-api/).
- Curious how it's built, or why it's shaped this way? See [Architecture](../architecture/overview/).

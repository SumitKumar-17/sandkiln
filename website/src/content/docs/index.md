---
title: sandkiln docs
description: Docs for sandkiln — a compute primitive for safely running untrusted or AI-generated code in hardware-isolated Firecracker microVMs.
---

Docs for sandkiln. Pick a starting point:

- **New here?** [Introduction](getting-started/introduction/) → [Self-hosting quickstart](getting-started/self-hosting/) → your language: [JS/TS](getting-started/js/), [Python](getting-started/python/), [CLI](getting-started/cli/).
- **Understand a capability:** [Core Concepts](concepts/sandbox-lifecycle/) — lifecycle, snapshots, naming, drives, images, networking, auth, preview.
- **Have a specific task:** [Guides](guides/run-untrusted-code/) — task-oriented, real-code recipes.
- **Need exact method signatures or HTTP routes:** [Reference](reference/http-api/) — daemon HTTP API, JS/TS SDK, Python SDK, CLI.
- **Curious how it's built:** [Architecture](architecture/overview/) — the four crates, the privilege model, the wire protocol, and the current startup-latency research.

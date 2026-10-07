---
title: sandkiln docs
description: Docs for sandkiln — a compute primitive for safely running untrusted or AI-generated code in hardware-isolated Firecracker microVMs.
---

Pick a starting point. Five ways into the same docs, by what you already know you need.

<div class="docs-start">
  <div class="docs-row">
    <span class="docs-row-label">New here</span>
    <span class="docs-row-body">
      <a href="getting-started/introduction/">Introduction</a> →
      <a href="getting-started/self-hosting/">Self-hosting quickstart</a> →
      your language: <a href="getting-started/js/">JS/TS</a>,
      <a href="getting-started/python/">Python</a>,
      <a href="getting-started/cli/">CLI</a>.
    </span>
  </div>
  <div class="docs-row">
    <span class="docs-row-label">Understand a capability</span>
    <span class="docs-row-body">
      <a href="concepts/sandbox-lifecycle/">Core Concepts</a> — lifecycle, snapshots,
      naming, drives, images, networking, auth, preview.
    </span>
  </div>
  <div class="docs-row">
    <span class="docs-row-label">Have a specific task</span>
    <span class="docs-row-body">
      <a href="guides/run-untrusted-code/">Guides</a> — task-oriented, real-code recipes.
    </span>
  </div>
  <div class="docs-row">
    <span class="docs-row-label">Need exact signatures</span>
    <span class="docs-row-body">
      <a href="reference/http-api/">Reference</a> — daemon HTTP API, JS/TS SDK, Python SDK, CLI.
    </span>
  </div>
  <div class="docs-row">
    <span class="docs-row-label">Curious how it's built</span>
    <span class="docs-row-body">
      <a href="internals/vsock-wire-protocol/">Internals</a> — one page per mechanism
      (vsock, the jailer, snapshots, sqlite, rate limiting, and nine more), each with a
      real request/response captured from the live daemon. Or the shorter
      <a href="architecture/overview/">Architecture</a> tour first, if you want the map
      before the terrain.
    </span>
  </div>
</div>

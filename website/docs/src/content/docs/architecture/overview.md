---
title: Overview
description: The Rust workspace, its four crates, and how a sandbox boots.
---

The core is a Rust workspace, kept deliberately modular — the daemon can change without touching the guest agent, and the guest agent ships as its own static binary independent of everything that talks to it.

## The four crates

- **`sandkiln-protocol`** — the wire format shared by host and guest: length-prefixed JSON, kept dependency-light so neither side reaches into the other.
- **`sandkiln-guest-agent`** — runs inside the microVM as a systemd service. Listens on vsock, answers exec/read/write/list — a static musl binary.
- **`sandkiln-vmm`** — drives Firecracker directly: boot, snapshot/resume, network leasing, drives, the vsock client. A hand-rolled HTTP client talks to Firecracker's own API socket.
- **`sandkiln-daemon`** — axum + tokio HTTP API wrapping the lifecycle (create, exec, list, stop, snapshot, images, drives) with auth, tags, and tracing built in from day one.

## Boot lifecycle

1. **Lease** — a tap device and IP are leased from the pool, attached to the bridge.
2. **Boot** — Firecracker starts; boot-source, drive, machine-config, and vsock are configured over its API socket.
3. **Agent up** — the guest kernel boots, systemd starts the guest agent, it binds its vsock port.
4. **Ready** — the host's vsock client connects; the sandbox can run anything sent to it.

Measured end to end: 32.3–33.1ms. See [Startup latency & the pre-warmed pool](../startup-latency/) for what's next after boot itself is already this fast.

## Deeper dives

Each of the following came from hitting a real constraint on real hardware, not a whiteboard preference:

- [Privilege model](../privilege-model/) — ambient `CAP_NET_ADMIN`, a static tap pool, and a root that stays out of the hot path.
- [Wire protocol](../wire-protocol/) — vsock, a length-prefixed JSON frame, and a client that doesn't reach for a full HTTP stack.
- [Persistence model](../persistence-model/) — sandbox vs. session, and why a snapshot points at paths instead of carrying values.
- [Bug hunt: the vsock timeout](../bug-hunt-vsock-timeout/) — case study of a real bug that could hang a stop forever.

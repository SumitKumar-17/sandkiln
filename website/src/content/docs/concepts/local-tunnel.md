---
title: Local tunnel
description: Expose a service on your own machine to code running inside a sandbox — the reverse of preview.
---

[Dev-server preview](../preview/) reaches a server running **inside** a sandbox from outside it. A local tunnel is the opposite direction: code running **inside** a sandbox reaches a service running on **your own machine** — a local dev server, a database, an internal API the sandboxed code needs to call during a test run.

## Opening a tunnel

- JS/TS: `const tunnel = await sandbox.tunnel(guestPort, { localPort })`
- Python: `tunnel = sandbox.tunnel(guest_port, local_port)` (or `await AsyncSandbox...`)
- CLI: `kiln sandbox tunnel <id> <guest-port> --local-port <port>` (runs in the foreground until `Ctrl+C`)

Once open, anything inside the sandbox that connects to `127.0.0.1:<guestPort>` actually reaches whatever's listening on `127.0.0.1:<localPort>` on the machine running the SDK — not a service inside the VM.

Call `tunnel.close()` (JS/Python) or stop the CLI process (`Ctrl+C`) when done. Closing tells the guest agent to stop listening and tears down the relay.

## Why this needed a new mechanism, not just another route

Every other vsock-based feature in sandkiln follows the same direction: the host connects in, the guest only ever listens. A tunnel's defining event — "something *inside* the guest just tried to connect" — can't be initiated by the host, by definition. This is the one place in the whole protocol where the guest dials out to the host instead, using a Firecracker vsock mechanism distinct from everything else here (no `CONNECT <port>` handshake — a pre-bound Unix socket the host listens on). See [the vsock wire protocol](../../internals/vsock-wire-protocol/) and [local tunnel: the mechanism](../../internals/local-tunnel/) for the full picture.

## Multiple connections, one tunnel

A tunnel's guest-side listener can accept more than one connection at a time (a sandboxed test suite opening several requests in parallel, say). Every one of them is multiplexed over the same single WebSocket back to the caller, each tagged with its own connection id — not a new WebSocket per connection.

## Auth

Unlike a plain HTTP API call, no WebSocket constructor in any runtime (browser or Node) can set a custom `Authorization` header — so like [PTY](../auth/) and dev-server preview, the tunnel's WebSocket accepts the auth token as a `?token=` query parameter instead. The `POST`/`DELETE` calls that create and close a tunnel use the normal header, since those go through a plain HTTP request.

## What's not done yet

Python's `tunnel()` is a hand-rolled WebSocket client (this package's first one) rather than a wrapper over an existing library — see [local tunnel: the mechanism](../../internals/local-tunnel/) for why, and what's deliberately out of scope (message fragmentation beyond what this protocol's own payload sizes ever produce).

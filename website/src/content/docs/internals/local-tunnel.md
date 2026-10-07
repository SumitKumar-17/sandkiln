---
title: "Local tunnel: the mechanism"
description: Why exposing a service on the caller's machine needed a guest-initiated vsock connection, the one exception to this protocol's own direction rule, and the hand-rolled WebSocket client that carries it the rest of the way.
---

## What it is

A local tunnel lets code running **inside** a sandbox reach a real TCP service running on the machine that called the SDK — the reverse of [dev-server preview](../../concepts/preview/), which goes the other way. `sandbox.tunnel(guestPort, { localPort })` makes `127.0.0.1:guestPort` inside the sandbox actually be `127.0.0.1:localPort` on your own machine, for as long as the returned handle stays open.

## Why sandkiln uses it here

Every other capability in this project follows one rule: the host connects to the guest, never the other way around (see [the vsock wire protocol](../vsock-wire-protocol/)). A tunnel's whole purpose breaks that rule structurally — the event that matters is "something *inside* the guest just tried to connect," which the host cannot have initiated, by definition. Building this meant finding the one place Firecracker's own vsock device actually supports the opposite direction, rather than working around the rule.

## Key terms

- **Guest-initiated vsock connection.** The direction `AGENT_PORT`/`PTY_PORT`/`EXEC_STREAM_PORT` never use: the guest calls `connect()` on its own `AF_VSOCK` socket toward the host, instead of the host dialing in.
- **`<uds_path>_<port>` listener.** Firecracker's actual mechanism for this direction, confirmed against Firecracker's own documentation rather than assumed: the host must pre-create and bind a plain Unix domain socket at the vsock device's own `uds_path` with `_<port>` appended. When the guest connects to that port number, Firecracker bridges it to whatever's listening there — a **different** mechanism from the `CONNECT <port>\n` text handshake the host-initiated direction uses, and with no handshake line of its own; the connection arrives already raw.
- **WebSocket frame multiplexing.** Since one tunnel can carry several concurrent connections (a sandboxed process opening more than one request at a time) but the SDK caller holds only one WebSocket open, every byte is tagged with a small frame identifying which logical connection it belongs to.
- **`conn_id` vs. `tunnel_id`.** A `tunnel_id` names one `guestPort` ↔ `localPort` mapping, created once by `POST /sandboxes/:id/tunnel`. A `conn_id` names one specific TCP connection accepted on that guest port — there can be many of these, created and destroyed throughout the tunnel's life, all multiplexed over the one WebSocket that `tunnel_id` owns.

## How it works in sandkiln

**Guest side** (`sandkiln-guest-agent::tunnel`): `StartTunnel { tunnel_id, guest_port }` (an ordinary `AGENT_PORT` request) binds a `TcpListener` on `127.0.0.1:guest_port` and accepts in the background. Each accepted connection dials out on `TUNNEL_PORT` (`VsockStream::connect_with_cid_port`), sends one framed `TunnelOpen { tunnel_id, conn_id }` — the only point in this whole flow that still uses this protocol's usual length-prefixed JSON framing — then becomes a raw byte passthrough, same shape as [PTY](../pty/)'s handshake-then-passthrough connections, including the identical `shutdown(Shutdown::Both)` fix for the two-cloned-handles hangup hazard [the engineering notebook](../../architecture/engineering-notebook/) documents for `pty.rs`.

**Host side** (`sandkiln-vmm::tunnel`): binds the `<uds_path>_<TUNNEL_PORT>` listener once per VM, right after boot or resume, and accepts for the VM's whole life. Every accepted connection reads its `TunnelOpen` handshake and is handed to whichever `tunnel_id` registered for it — a plain `std::sync::mpsc` channel, not a tokio one, since this crate has no async runtime dependency by design, the same reasoning `vsock_client.rs` already follows for every other vsock call this crate makes.

**Daemon side** (`routes_tunnel.rs`): `POST /sandboxes/:id/tunnel` registers a fresh `tunnel_id` and tells the guest to start listening. `GET /sandboxes/:id/tunnel/:tunnel_id/ws` is the WebSocket the caller holds open, guarded by the same `?token=`-accepting middleware as [PTY](../pty/) — no WebSocket constructor in any runtime can set a custom header, which is also why the `POST`/`DELETE` calls above use the normal bearer token but this one can't. The daemon multiplexes every connection that arrives for that tunnel over it using one binary frame format: one `op` byte (`open`/`data`/`close`), one length byte, that many bytes of `conn_id`, then payload.

**Caller side** (JS `tunnel.ts`, Python `tunnel.py`/`atunnel.py`): decodes that same frame format and, for every `open`, connects a real local socket to `localPort`, forwarding `data` both directions until a `close`. The JS implementation rides on the runtime's native `WebSocket`; Python has none in its standard library, so `tunnel.py` hand-rolls the RFC 6455 handshake and frame masking directly — verified in review against the RFC's own published test vector (key `dGhlIHNhbXBsZSBub25jZQ==` → accept `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`), not just "it connects to a real server."

## See it in action

A real tunnel, opened against a live daemon, relaying a real HTTP response from a Node server on the host into a `curl` running inside the sandbox:

```
$ curl -s -X POST http://127.0.0.1:7777/sandboxes/c63766c1-aec7-4231-9986-562f443d653f/tunnel \
    -H 'content-type: application/json' -d '{"guest_port": 8080}'
{"tunnel_id":"238565ce-3163-4bb5-8f4a-db4ee1a8032f","guest_port":8080}
```

Then, from inside that same sandbox, with the tunnel open:

```
$ curl -s http://127.0.0.1:8080/
hello from the real local machine, 1791345337714
```

That response was never generated inside the VM — it's the exact string a plain `http.createServer` on the host machine returned, reached entirely through the tunnel. See [`examples/local-tunnel`](https://github.com/SumitKumar-17/sandkiln/tree/main/examples/local-tunnel) for the full, runnable version of this (the local server, the tunnel, the `curl`, and the byte-for-byte comparison), and the Python/CLI equivalents, which were verified the same way.

## What's not done yet

Python's WebSocket client doesn't handle message fragmentation (a single logical message split across multiple WebSocket frames) — a deliberate scope decision, not an oversight: every payload this protocol ever sends is capped well under any size where a real server would start fragmenting, so a minimal non-fragmenting reader is correct for this specific use, not a general-purpose WebSocket client. Building a fully spec-complete client was explicitly out of scope given that constraint.

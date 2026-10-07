# Local tunnel

## What it is

The reverse of dev-server preview: `sandbox.tunnel(guestPort, { localPort })` makes `127.0.0.1:guestPort` *inside* the sandbox actually be `127.0.0.1:localPort` on the machine running the SDK. Code running inside the sandbox reaches a real service on the caller's own machine — a local dev server, a database, an internal API.

## The core design problem

Every other vsock-based capability in this project (exec, file ops, PTY, streamed exec) follows one direction without exception: the host connects to the guest, never the reverse (see [06-vsock-protocol.md](06-vsock-protocol.md)). A tunnel's defining event — "something *inside* the guest just tried to connect" — can't be initiated by the host, by definition. This is the one deliberate exception to that rule in the whole stack.

**The mechanism, confirmed against Firecracker's own documentation before building** (an earlier design sketch assumed it was just the host-initiated `CONNECT <port>` handshake read in reverse — it is not): the host pre-creates and binds a plain Unix socket at the vsock device's own `uds_path` with `_<port>` appended (`v.sock_5003`), before the guest ever connects. When the guest calls `connect()` toward that port, Firecracker bridges it straight to that socket — no handshake line of its own. A small application-level handshake (`TunnelOpen { tunnel_id, conn_id }`) rides on top of that raw bridge so the host can tell which tunnel/connection it just received.

## Layer by layer

- **Protocol** (`sandkiln-protocol`): `TUNNEL_PORT` (5003), `Request::StartTunnel`/`StopTunnel` (plain `AGENT_PORT` request/response), `TunnelOpen` (the one framed message on `TUNNEL_PORT`).
- **Guest agent** (`tunnel.rs`): `StartTunnel` binds a `TcpListener` on the guest port; each accepted connection dials `TUNNEL_PORT` out to the host, sends `TunnelOpen`, then relays bytes — same `shutdown(Shutdown::Both)` two-cloned-handles fix as `pty.rs`.
- **vmm** (`tunnel.rs`): binds the `<uds_path>_<TUNNEL_PORT>` listener once per VM at boot/resume; dispatches each accepted connection to whichever `tunnel_id` registered for it via a plain `std::sync::mpsc` channel (no tokio dependency in this crate, by design).
- **Daemon** (`routes_tunnel.rs`): `POST /sandboxes/:id/tunnel` registers and starts the guest listener; `GET .../tunnel/:tunnel_id/ws` is the long-lived WebSocket the caller holds, multiplexing every guest connection over it with a small binary frame (`op` byte + `conn_id` + payload). The WebSocket route needs the `?token=` auth fallback (no WebSocket constructor in any runtime can set a header); the `POST`/`DELETE` calls use the normal bearer token.
- **SDKs**: JS/TS rides the runtime's native `WebSocket`. Python has none in its standard library — `tunnel.py`/`atunnel.py` hand-roll the RFC 6455 handshake and frame masking directly, verified against the RFC's own published test vector before trusting it live.
- **CLI**: `kiln sandbox tunnel <id> <guest-port> --local-port <port>`, foreground until `Ctrl+C`.

## Status

Done, live-verified end to end on all three surfaces (JS SDK, Python SDK, CLI) — a real local HTTP server reached from inside a real sandbox via `curl`, byte-for-byte matching. See [`website/src/content/docs/internals/local-tunnel.md`](../../website/src/content/docs/internals/local-tunnel.md) for the full mechanism writeup with a real captured example, and `examples/local-tunnel/` for the runnable reference.

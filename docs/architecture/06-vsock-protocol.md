# The vsock wire protocol

## What it is

`AF_VSOCK` is a Linux socket address family built for hypervisor↔guest
communication with **no IP stack involved at all** — no ARP, no interface to
bring up inside the guest. Firecracker exposes it as one virtio-vsock device
per VM, backed on the host by a single Unix domain socket path; a host process
connects to that path and sends a short handshake line naming which guest port
it wants, and Firecracker bridges the rest of the connection straight to a
listener inside the guest on that port.

## Why sandkiln uses it

The guest agent needs to receive commands and send output without exposing an
extra listening port on the same network interface the sandboxed workload's own
traffic uses (or building a whole second isolated guest network just for
control traffic). vsock sidesteps this: it's not IP traffic, so it's
unreachable from anything the sandboxed workload can touch, and a `deny_all`
egress policy has no bearing on it — it was never subject to that policy in the
first place.

## Framing: length-prefixed JSON

`sandkiln-protocol`, depended on by both `sandkiln-vmm` and
`sandkiln-guest-agent` so the two sides can never disagree on message shape.
Every message: a 4-byte little-endian length prefix, then exactly that many
bytes of JSON. Deliberately **not** newline-delimited — `exec` output can
contain arbitrary bytes including embedded newlines and nulls, and a length
prefix is the one framing scheme that kind of payload can't corrupt by
accident. JSON itself is boring on purpose: no schema compiler needed for a
protocol this small, readable in a packet capture, and `#[serde(tag = "cmd")]`
on the `Request`/`Response` enums keeps message shapes self-describing.

## Three connection shapes, one port each

- **`AGENT_PORT` (5000).** One request in, one response out, then close.
  `exec`, `read_file`, `write_file`, `chmod`, and the rest of the file
  operations. The daemon merges create-time `env` defaults and any per-call
  `env` override into one final map *before* sending this message — the guest
  agent has no notion of two layers, only one already-merged map handed to it.
- **`PTY_PORT` (5001).** One framed `PtyHandshake { cols, rows }`, then a raw,
  unframed byte passthrough for the connection's whole life — an interactive
  shell's input/output isn't discrete messages, and framing it would just add
  overhead to something meant to feel like a real terminal.
- **`EXEC_STREAM_PORT` (5002).** Framed messages the whole way through (unlike
  `PTY_PORT`), but one long-lived connection per session (unlike `AGENT_PORT`'s
  one-shot request/response) — for streaming a long-running command's output
  as it's produced.

## The direction invariant

**The host always connects in; the guest agent only ever listens.** Every one
of the three ports above follows this — `vmm::vsock_client` is the thing that
dials `UnixStream::connect(uds_path)` and sends the `CONNECT <port>` handshake
Firecracker's own vsock proxy expects. This is a deliberate architectural
choice (one mental model for every port) rather than an accident, and it's the
reason a feature needing the *opposite* direction (something initiated from
inside the guest reaching back out to the host) is a genuinely novel addition,
not a small extension.

## Status

Done, live-verified. See [`website/src/content/docs/internals/vsock-wire-protocol.md`](../../website/src/content/docs/internals/vsock-wire-protocol.md)
for a real captured `exec` request's exact bytes.

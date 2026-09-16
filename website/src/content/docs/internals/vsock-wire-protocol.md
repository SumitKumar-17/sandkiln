---
title: vsock and the host-guest wire protocol
description: What AF_VSOCK is, why sandkiln's guest protocol is a length-prefixed JSON frame, and the real bytes one exec sends over it.
---

## What it is

`AF_VSOCK` is a Linux socket address family built for one purpose: letting a hypervisor host and one of its guest VMs talk to each other without a network. A normal socket between two machines needs an IP address, a route, and something listening on a port that's reachable over that route. A vsock socket needs none of that. The hypervisor itself (Firecracker, in this case) wires up the channel directly between a host-side Unix domain socket and a port number inside the guest kernel, so there's no IP stack involved on either side of that specific channel, no ARP, no interface to bring up.

Firecracker exposes vsock as a single virtio-vsock device per VM, backed on the host by one Unix domain socket path. A process on the host connects to that path and sends a short handshake line naming which guest port it wants; Firecracker then bridges the rest of that TCP-like connection straight to a listener inside the guest on that port.

## Why sandkiln uses it here

The guest agent (`sandkiln-guest-agent`, a small binary baked into every sandbox's rootfs) needs to receive commands from the daemon and send back output: exec a process, read or write a file, open a PTY. Doing that over the sandbox's own network interface would mean either exposing an extra listening port on the same interface a caller's own outbound traffic uses, or building a second, isolated guest network just for control traffic. vsock sidesteps the whole problem. It's a channel the guest agent listens on that has nothing to do with the sandbox's network configuration at all, doesn't need `enp0s...`-style interface setup inside the guest, and isn't reachable from anything the sandboxed workload itself can touch, since it was never IP traffic to begin with. A `deny_all` egress policy (see [iptables and per-sandbox egress policy](../egress-iptables/)) has no bearing on it for the same reason.

## Key terms

- **AF_VSOCK.** The Linux socket address family for hypervisor-guest communication, distinct from `AF_INET`/`AF_INET6`.
- **CID (context ID).** vsock's equivalent of an IP address: an integer identifying one side of the channel. Firecracker manages this internally; sandkiln's own code never has to construct or parse a CID directly, only the Unix domain socket path and the port.
- **Guest port.** A plain integer the guest-side listener binds, analogous to a TCP port but scoped only to vsock traffic for that one VM.
- **Framing.** The scheme that tells a reader where one message ends and the next begins, on a byte stream that has no built-in message boundaries of its own.

## How it works in sandkiln

`sandkiln-protocol` (a small crate depended on by both the host-side `sandkiln-vmm` and the guest-side `sandkiln-guest-agent`, so the two sides can never define the message shapes differently) defines the actual framing: every message is a 4-byte little-endian length prefix, followed by exactly that many bytes of JSON. It's deliberately not newline-delimited, since `exec` output can contain arbitrary bytes, including embedded newlines and even embedded null bytes, and a length prefix is the one framing scheme a payload like that can't corrupt by accident. JSON itself is a deliberate, boring choice on top of that: no schema compiler needed for a protocol this small, readable directly in a packet capture while debugging, and a tagged `cmd`/`status` field on the `Request`/`Response` enums (`#[serde(tag = "cmd")]` on the Rust side) keeps the handful of message shapes self-describing.

There are three separate vsock ports, not one, because the three kinds of traffic have genuinely different shapes:

- **`AGENT_PORT` (5000).** One request in, one response out, then the connection can close. This is where `exec`, `read_file`, `write_file`, `chmod`, and the rest of the file-operation calls all go, each as one `Request` variant and one matching `Response`.
- **`PTY_PORT` (5001).** A long-lived connection that sends exactly one framed handshake (`PtyHandshake { cols, rows }`), then drops into a raw, unframed byte passthrough for the rest of its life. An interactive shell's input and output aren't discrete messages, and framing them would just add overhead to a channel that's supposed to feel like a real terminal.
- **`EXEC_STREAM_PORT` (5002).** Also long-lived, but unlike `PTY_PORT` it never stops being framed: after one `ExecStreamHandshake`, every subsequent message is a framed `ExecStreamEvent` (`Stdout`, `Stderr`, or one final `Exit` carrying the process's exit code). A caller watching a detached background command needs to know *which* stream a chunk came from and when the process actually exited, not just an undifferentiated blob of bytes.

Firecracker's own control plane is a related but separate problem, solved differently: the daemon configures boot-source, drives, and machine-config, then later pauses, snapshots, and resumes a VM. That traffic goes over a *different* socket (Firecracker's own API Unix socket, not vsock) using plain HTTP/1.1, but a full HTTP client crate would buy nothing there either, since no redirects, chunked encoding, or TLS will ever be exercised against a socket that speaks exactly one small, fixed JSON PUT/PATCH API. So `sandkiln-vmm` hand-rolls the roughly ninety lines a minimal client actually needs: write the request line and body, read the status line, read headers looking only for `Content-Length`, then read exactly that many bytes back.

## See it in action

A real `exec` call, captured live against a running daemon (`curl -i`, full headers included):

```
$ curl -s -i -X POST http://127.0.0.1:7777/sandboxes/354dbc49-.../exec \
    -H 'content-type: application/json' \
    -d '{"command":"echo","args":["hello-vsock"]}'

HTTP/1.1 200 OK
content-type: application/json
x-request-id: 108661cc-a6b0-40ed-aa4a-dc22cb4c07a6
content-length: 52
date: Wed, 16 Sep 2026 05:24:32 GMT

{"stdout":"hello-vsock\n","stderr":"","exit_code":0}
```

That HTTP round trip is the daemon's own public API. The vsock traffic it triggers underneath is invisible to the caller, by design. What the daemon actually sends the guest agent over `AGENT_PORT`, in the exact wire format `sandkiln-protocol` defines, for that same call:

```
request:  67 bytes on the wire = 4-byte length prefix (0x3f 0x00 0x00 0x00, i.e. 63) + this JSON:
          {"cmd":"exec","command":"echo","args":["hello-vsock"],"env":{}}

response: 72 bytes on the wire = 4-byte length prefix (0x44 0x00 0x00 0x00, i.e. 68) + this JSON:
          {"status":"exec","stdout":"hello-vsock\n","stderr":"","exit_code":0}
```

The `env` field is always present, even empty, because the daemon resolves a sandbox's create-time environment-variable defaults and any per-call override into one final map before it ever builds this message; the guest agent has no notion of the two layers, only the one map it's handed.

# AGENTS.md: sandkiln-protocol

Read root `AGENTS.md` first. The wire format shared by `sandkiln-vmm` and `sandkiln-guest-agent`, so
message shapes and vsock port numbers cannot drift. Only `serde` and `serde_json`; no knowledge of vsock,
HTTP or Firecracker. Process and networking needs belong in `vmm` or `guest-agent`.

## Files

- **`messages.rs`**: `Request`/`Response` enums (`Exec`, `ReadFile`, `WriteFile`, `ListDir`, `Chmod`,
  `Chown`, `Mkdir`, `Rename`, `Copy`, `Symlink`, `Readlink`, `Truncate`, `StartTunnel`, `StopTunnel`),
  `DirEntry`, and the non-request messages: `PtyHandshake { cols, rows }` (a PTY session is one framed
  handshake, then raw bytes), `ExecStreamHandshake` with `ExecStreamEvent` (`Stdout`/`Stderr`/`Exit`,
  framing never stops, and the exit code is part of the contract), and `TunnelOpen { tunnel_id, conn_id }`.
  `env` on `Exec` and `ExecStreamHandshake` is already merged by the daemon (`resolve_env`).
- **`framing.rs`**: 4-byte LE length prefix plus payload, so binary content cannot be mistaken for a frame
  boundary. Also frames `PtyHandshake` and `TunnelOpen`.
- **`lib.rs`**: re-exports, `AGENT_PORT`, `PTY_PORT`, `EXEC_STREAM_PORT`, `TUNNEL_PORT`, and
  `encode_*`/`decode_*` helpers that keep `serde_json` an implementation detail. `TUNNEL_PORT` is the one
  exception to "host connects in, guest listens" (guest-initiated connection to a `<uds_path>_<port>` listener).

## Adding an operation

New variant in `messages.rs` (with a wire-shape test and an entry in `every_request_variant_roundtrips`),
implement it in `guest-agent`'s `handler.rs`, expose it from `daemon` (`http/routes/exec.rs` for data
transfer, `fs.rs` for filesystem structure), then SDKs and CLI.

## Compatibility

No version negotiation: `guest-agent` and `vmm`/`daemon` must be rebuilt and redeployed together, and the
guest agent re-injected into every rootfs. The compiler flags missing `match` arms only if both sides are rebuilt.

## Testing

`cargo test -p sandkiln-protocol` (framing and round-trips). A behavioral change also needs a live boot
against the guest agent and daemon.

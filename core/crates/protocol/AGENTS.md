# AGENTS.md — sandkiln-protocol

Read root `AGENTS.md` first.

## What this crate is

The wire format shared by the host (`sandkiln-vmm`) and guest
(`sandkiln-guest-agent`) — both depend on it so message shapes and the
vsock port numbers can never silently drift apart.

Dependency-light (`serde` + `serde_json` only), zero knowledge of vsock,
HTTP, or Firecracker — just messages and framing. A networking or
process-related need belongs in `vmm`/`guest-agent`, not here.

## Files

- **`messages.rs`** — `Request`/`Response` enums: `Exec`, `ReadFile`,
  `WriteFile`, `ListDir`, `Chmod`, `Chown`, `Mkdir`, `Rename`, `Copy`,
  `Symlink`, `Readlink`, `Truncate`, plus `DirEntry` (a `ListDir` entry's
  metadata). New operation → new variant here (with a wire-shape test and
  an entry in `every_request_variant_roundtrips`) → implement in
  `guest-agent`'s `handler.rs` → expose via `daemon`'s `routes_exec.rs`
  (data transfer) or `routes_fs.rs` (filesystem structure).
  `Exec`/`ExecStreamHandshake`'s `env` is already fully resolved by the
  daemon (`routes_exec::resolve_env`) before it reaches this crate — no
  sandbox-level/call-level distinction here, just one final map.
  Also defines `PtyHandshake { cols, rows }` (not a `Request`/`Response`
  variant — a PTY session isn't request/response shaped; one framed
  handshake, then raw bytes, see `guest-agent`'s `pty.rs` and `daemon`'s
  `routes_pty.rs`) and `ExecStreamHandshake { command, args }` +
  `ExecStreamEvent` (`Stdout`/`Stderr`/`Exit`, internally tagged like
  `Response`) for `EXEC_STREAM_PORT` — framing never stops here, a
  structured exit code is part of the contract (see `exec_stream.rs` /
  `routes_logs.rs`). Also defines `Request::StartTunnel { tunnel_id,
  guest_port }`/`StopTunnel { tunnel_id }` (plain request/response over
  `AGENT_PORT`, for the local tunnel feature) and `TunnelOpen { tunnel_id,
  conn_id }` (the one framed handshake a guest-initiated `TUNNEL_PORT`
  connection sends — see `TUNNEL_PORT`'s own doc comment).
- **`framing.rs`** — length-prefixed framing (4-byte LE length + payload),
  not newline-delimited, so binary file content can never be
  misinterpreted as a frame boundary. Also what frames `PtyHandshake`/
  `TunnelOpen`, the only framed messages on `PTY_PORT`/`TUNNEL_PORT`.
- **`lib.rs`** — re-exports, `AGENT_PORT`/`PTY_PORT`/`EXEC_STREAM_PORT`/
  `TUNNEL_PORT`, and `encode_*`/`decode_*` helpers keeping `serde_json` an
  implementation detail. `TUNNEL_PORT` is the one deliberate exception to
  this protocol's universal "host connects in, guest only listens" rule —
  see its own doc comment for why and how (Firecracker's guest-initiated-
  connection mechanism, a `<uds_path>_<port>` listener, not the
  `CONNECT <port>` handshake the other three ports use).

## Changing the protocol

Both `guest-agent` and `vmm`/`daemon` need rebuilding and
redeploying together — no version negotiation. A new variant needs both
crates' `match` statements updated (`handler.rs`; `routes_exec.rs`/
`routes_fs.rs`) — the compiler catches missing arms only if you actually
rebuild both sides.

## Verifying a change

No live-testable behavior here (types + pure framing) — `cargo
build`/`clippy -p sandkiln-protocol` on the remote dev box is the bar. A
behavioral change needs the full guest-agent + daemon live-boot
verification (root `AGENTS.md`).

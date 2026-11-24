# AGENTS.md: sandkiln-guest-agent

Read root `AGENTS.md` first. A small static binary (`x86_64-unknown-linux-musl`, size-optimized in
the workspace `Cargo.toml`) that runs inside every microVM as a systemd service, not PID 1. It is
the only code that runs in the guest. It serves four vsock ports:

- `AGENT_PORT`: framed request/response (exec, file and filesystem ops).
- `PTY_PORT`: interactive shell; one handshake, then raw bytes.
- `EXEC_STREAM_PORT`: streamed background exec; framed throughout.
- `TUNNEL_PORT`: the one port it dials out on instead of listening (see `sandkiln_protocol::TUNNEL_PORT`).

## Files

- **`main.rs`**: listener loops. `AGENT_PORT` reads framed messages until disconnect and dispatches to
  `handler::handle`. PTY and exec-stream accept and spawn a thread per session so a long session never
  blocks `accept()`.
- **`handler.rs`**: each `Request` as a thin wrapper over `std::process::Command`/`std::fs`, plus one
  raw `libc::chown`. No path validation, deliberately: paths resolve inside that one microVM.
- **`pty.rs`**: reads `PtyHandshake`, `forkpty(2)` a shell of that size, shovels bytes between the vsock
  connection and the pty master on two threads. See the hangup invariant below.
- **`exec_stream.rs`**: reads `ExecStreamHandshake`, spawns with piped stdout/stderr (no pty), fans both
  pipes into one channel so only the connection thread writes `ExecStreamEvent`s, waits for the child
  only after both pipes hit EOF (waiting first can deadlock on a full pipe), then sends `Exit`.
- **`tunnel.rs`**: `start(tunnel_id, guest_port)` binds `127.0.0.1:guest_port`; each accepted connection
  dials `TUNNEL_PORT` on the host CID, sends a framed `TunnelOpen`, then relays bytes. `stop` sets a flag
  and connects to its own listener to unblock `accept()`. A process-wide registry tracks tunnels by id.

## Invariants

- The two vsock handles in `pty.rs` are `try_clone()`d fds on one socket, so dropping one does not close
  it and a blocked read on the other waits forever. `shovel_bytes` shuts the socket down
  (`Shutdown::Both`) when the shell exits, and sends the child `SIGHUP` if the vsock side ends first.
  `tunnel.rs`'s `relay` applies the same shutdown pattern.
- No recovery inside a connection: a bad frame ends that connection rather than resyncing.
- Do not assume unrestricted root; jailer hardening may restrict this process later.

## Build, inject, verify

```
cargo build --release -p sandkiln-guest-agent --target x86_64-unknown-linux-musl   # remote dev box
scripts/dev.sh inject-agent [rootfs-path]      # build then inject; defaults to SANDKILN_BASE_ROOTFS
```

Building is not enough: the binary must be baked into a rootfs (`images/inject-agent.sh` copies it to
`/usr/local/bin/` and enables the service), and `base_rootfs` must point at that image or the daemon keeps
booting the old one. Verify with a full build, inject into a fresh rootfs copy, boot, and talk over vsock.

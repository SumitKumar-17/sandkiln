# AGENTS.md — sandkiln-guest-agent

Read root `AGENTS.md` first.

## What this crate is

A ~700KB static binary running *inside* every microVM as a systemd
service, listening on four vsock ports: `AGENT_PORT` (request/response —
exec, file ops, chmod/chown/mkdir/rename/copy/symlink/readlink/truncate),
`PTY_PORT` (interactive shell, long-lived, drops to raw bytes after one
handshake — see `pty.rs`), `EXEC_STREAM_PORT` (streamed background exec,
long-lived but stays framed the whole time — see `exec_stream.rs`),
`TUNNEL_PORT` (local tunnel — see `tunnel.rs`, the one port this crate
*dials out on* instead of listening, per `TUNNEL_PORT`'s own doc
comment). The only code that runs inside the guest; everything else is
host-side.

Built for `x86_64-unknown-linux-musl` (static, no glibc-version
dependency). Size-optimized in the workspace root `Cargo.toml` since it
ships inside every rootfs image.

## Files

- **`main.rs`** — three listener loops. `AGENT_PORT`: accept, read framed
  messages in a loop, dispatch to `handler::handle`, write response,
  repeat until disconnect. `PTY_PORT`/`EXEC_STREAM_PORT`: accept and spawn
  a thread per session (`pty::handle_connection`/
  `exec_stream::handle_connection`) so a long session never blocks the
  next `accept()`.
- **`handler.rs`** — each `Request` variant, as thin wrappers over
  `std::process::Command`/`std::fs` plus one raw `libc::chown` (std has
  no equivalent; `libc` over `nix` for one syscall). No path validation
  of any kind, deliberately — the guest agent is a dumb executor, a path
  is scoped to whatever it resolves to inside that one microVM's own
  filesystem regardless.
- **`pty.rs`** — read `PtyHandshake`, `forkpty(2)` a shell sized to it
  (via `nix`), shovel bytes between the vsock connection and pty master
  on two threads until either side ends. See the hangup gotcha below.
- **`exec_stream.rs`** — read `ExecStreamHandshake`, spawn with
  stdout/stderr piped (no pty), fan both pipes into one channel so only
  the connection thread writes framed `ExecStreamEvent`s (no writer
  synchronization needed), wait for the child only after both pipes hit
  EOF (waiting first risks deadlocking on a full pipe buffer), send one
  final `Exit`.
- **`tunnel.rs`** — local tunnel: `start(tunnel_id, guest_port)` binds a
  `TcpListener` on `127.0.0.1:guest_port` and accepts in the background;
  each accepted connection dials `TUNNEL_PORT` (`VsockStream::
  connect_with_cid_port(VMADDR_CID_HOST, ...)`), sends one framed
  `TunnelOpen`, then shovels bytes until either side ends — same
  `try_clone()`-on-one-socket hangup hazard and `shutdown(Shutdown::Both)`
  fix as `pty.rs`'s `shovel_bytes`. `stop(tunnel_id)` sets a flag and
  connects to the listener itself to unblock its `accept()` (std has no
  other way to cancel a blocking accept). A process-wide registry
  (`OnceLock<Mutex<HashMap<...>>>`) tracks active tunnels by id.

## Building

```
cargo build --release -p sandkiln-guest-agent --target x86_64-unknown-linux-musl
```
On the remote dev box (needs the musl target + `musl-tools`).

## Getting a change into a real microVM

Building isn't enough — it has to be baked into a rootfs image.
`scripts/dev.sh inject-agent [rootfs-path]` does build-then-inject in one
command (defaults to `SANDKILN_BASE_ROOTFS`) — safer than the two steps
separately (a wrong-image injection happened once during development):
```
sudo bash images/inject-agent.sh \
  core/target/x86_64-unknown-linux-musl/release/sandkiln-agent \
  <path-to-rootfs.ext4>
```
Mounts the image, copies the binary to `/usr/local/bin/`, enables the
systemd service. `SANDKILN_BASE_ROOTFS` must point at whatever you
injected into, or the daemon keeps booting the old image.

## Non-obvious things

- No error recovery inside a connection — a bad frame or serialize
  failure just ends that connection, deliberately, rather than resyncing.
- Runs as a regular systemd service, not PID 1 — don't assume init
  semantics.
- A capability needing more system access here should assume Firecracker
  jailer hardening (see root `ROADMAP.md`) will eventually restrict it —
  don't build in an unrestricted-root assumption.
- **`pty.rs`'s two vsock handles are `try_clone()`d, dup'd fds on one
  socket, not two connections** — dropping one doesn't close it, so a
  blocked read on the other would wait forever. `shovel_bytes` handles
  both hangup directions explicitly: the pty-output thread calls
  `shutdown(Shutdown::Both)` on shell exit; the vsock-input side sends
  the child `SIGHUP` (same as a real terminal) if it ends first. This was
  a real 10-second-hang bug, only ever caught via a live CLI test.
  `tunnel.rs`'s `relay` function applies the identical `shutdown(Shutdown::Both)`
  pattern proactively (no process to `SIGHUP` here, just two plain
  sockets) — built in from the start, not found the hard way twice.

## Verifying a change

Compiling isn't proof — needs the full build → inject into a fresh
rootfs copy → boot → talk over vsock loop (root `AGENTS.md`).

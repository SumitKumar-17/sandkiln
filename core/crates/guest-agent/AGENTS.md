# AGENTS.md — sandkiln-guest-agent

Read the root `AGENTS.md` first for project-wide conventions. This file
is scoped to this one crate.

## What this crate is

A ~700KB static binary that runs *inside* every microVM as a systemd
service, listening on two vsock ports: `sandkiln_protocol::AGENT_PORT`,
answering `Request`s from `sandkiln-protocol` (exec, read/write file,
directory listing with metadata, chmod/chown/mkdir/rename/copy/symlink/
readlink/truncate), and `sandkiln_protocol::PTY_PORT`, for interactive
shell sessions (see `pty.rs` below) — a fundamentally different,
long-lived-raw-bytes shape from the first port's one-request-one-response
traffic. This is the only code that ever runs inside the guest —
everything else (`vmm`, `daemon`) is host-side.

Built for `x86_64-unknown-linux-musl` specifically (static linking, no
libc dependency on the guest's exact glibc version) — see root
`AGENTS.md`'s note on this. Optimized for size in the workspace root
`Cargo.toml` (`[profile.release.package.sandkiln-guest-agent]`) since it
ships inside every rootfs image and directly affects image size and boot
time.

## Files

- `main.rs` — two listener loops, one per port, the `PTY_PORT` one on
  its own thread from startup: the `AGENT_PORT` loop accepts a
  connection, reads framed messages in a loop, dispatches to
  `handler::handle`, writes the framed response, repeats until the peer
  disconnects; the `PTY_PORT` loop accepts a connection and spawns a new
  thread per session (`pty::handle_connection`), since a PTY session is
  expected to stay open a long time and must never block the next
  `accept()`.
- `handler.rs` — the actual implementation of each `Request` variant.
  This is genuinely simple (thin wrappers over `std::process::Command`
  and `std::fs`, plus one raw `libc::chown` call — std has no chown
  equivalent, and `libc` was chosen over `nix` as the one dependency
  needed for a single syscall wrapper) by design — don't add business
  logic here that belongs on the host side instead. The guest agent
  should stay a dumb executor: no path validation of any kind on any
  operation (not `..`-rejection, not an absolute-path requirement, not
  canonicalization) — whatever the guest's own kernel permits, this
  does. That's deliberate, not a gap to fix here; a path is scoped to
  whatever it resolves to inside that one microVM's own filesystem
  regardless.
- `pty.rs` — one interactive PTY session start to finish: read the
  `PtyHandshake`, `forkpty(2)` a shell sized to it (via `nix`, gated
  behind its `term`/`process`/`signal` features), then shovel bytes
  between the vsock connection and the pty master on two threads until
  either side ends. See "Non-obvious things" below for the one real
  gotcha in that last part.

## Building

```
cargo build --release -p sandkiln-guest-agent --target x86_64-unknown-linux-musl
```
on the remote dev box (needs the musl target + `musl-tools` installed —
see `scripts/` on the dev box or just `rustup target add
x86_64-unknown-linux-musl` + `apt install musl-tools` if starting fresh).

## Getting a change into a real microVM

Building the binary isn't enough — it has to be baked into a rootfs
image before it does anything. `scripts/dev.sh inject-agent
[rootfs-path]` does the build-then-inject sequence below in one command
(defaulting `rootfs-path` to the daemon's own configured
`SANDKILN_BASE_ROOTFS` default) — worth using over the two steps
separately specifically because it can't inject into the wrong image
file, a real mistake made at least once during this crate's own
development:
```
sudo bash images/inject-agent.sh \
  core/target/x86_64-unknown-linux-musl/release/sandkiln-agent \
  <path-to-rootfs.ext4>
```
This mounts the image, copies the binary to `/usr/local/bin/`, and
enables the systemd service. The daemon's `SANDKILN_BASE_ROOTFS` env var
needs to point at whatever image you injected into, or it'll keep
booting sandboxes from the old one.

## Non-obvious things

- **No error recovery inside a connection.** If a request fails to parse
  or a response fails to serialize, `main.rs` just ends that connection
  — it does not try to resync the stream. This is deliberate simplicity,
  not an oversight; a malformed frame means something is wrong enough
  that resyncing isn't worth the complexity.
- **This binary is PID-independent of systemd's actual PID 1** — it runs
  as a regular systemd service (`sandkiln-agent.service`), not as init
  itself. Don't assume PID 1 semantics.
- If you add a capability here that needs more system access (mounting,
  privileged syscalls), remember Firecracker's jailer hardening (planned,
  see root `ROADMAP.md`) will eventually restrict what this process can
  do — don't build in an assumption of unrestricted root that a future
  security pass will have to unwind.
- **`pty.rs`'s two vsock-stream handles are `try_clone()`d, not two
  independent connections** — they're dup'd fds sharing the *same*
  underlying socket. Dropping just one of them on a thread's own exit
  does **not** close the connection (the kernel keeps a socket open as
  long as any fd still references it), so a blocked read on the other
  handle would otherwise wait forever for bytes nobody is left to send.
  `shovel_bytes` handles both hangup directions explicitly instead of
  assuming either side notices on its own: the pty-output thread calls
  `stream.shutdown(Shutdown::Both)` when the shell exits (unblocking the
  vsock-input thread's read immediately), and the vsock-input side sends
  the child `SIGHUP` when *it* ends first — the same signal a real
  terminal sends its foreground process group on hangup — so a
  disconnected session never leaves an orphaned shell running. This was
  a real bug (a 10-second hang, only ever noticed via a live CLI test,
  not a unit test) — see `packages/cli`'s `sandbox pty` command for
  where it first showed up.

## Verifying a change

Compiling isn't proof it works — this crate specifically needs the full
live-boot verification loop (build → inject into a fresh rootfs copy →
boot via `scripts/dev-tools/boot-test-vm.sh` or the daemon → talk to it over vsock)
described in the root `AGENTS.md`. A change here that only "compiles" has
not been verified.

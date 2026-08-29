# AGENTS.md — sandkiln-guest-agent

Read the root `AGENTS.md` first for project-wide conventions. This file
is scoped to this one crate.

## What this crate is

A ~700KB static binary that runs *inside* every microVM as a systemd
service, listening on vsock port `sandkiln_protocol::AGENT_PORT` and
answering `Request`s from `sandkiln-protocol` (exec, read/write file,
list directory). This is the only code that ever runs inside the guest —
everything else (`vmm`, `daemon`) is host-side.

Built for `x86_64-unknown-linux-musl` specifically (static linking, no
libc dependency on the guest's exact glibc version) — see root
`AGENTS.md`'s note on this. Optimized for size in the workspace root
`Cargo.toml` (`[profile.release.package.sandkiln-guest-agent]`) since it
ships inside every rootfs image and directly affects image size and boot
time.

## Files

- `main.rs` — the vsock listener loop: accept a connection, read framed
  messages in a loop, dispatch to `handler::handle`, write the framed
  response, repeat until the peer disconnects.
- `handler.rs` — the actual implementation of each `Request` variant.
  This is genuinely simple (thin wrappers over `std::process::Command`
  and `std::fs`) by design — don't add business logic here that belongs
  on the host side instead. The guest agent should stay a dumb executor.

## Building

```
cargo build --release -p sandkiln-guest-agent --target x86_64-unknown-linux-musl
```
on the remote dev box (needs the musl target + `musl-tools` installed —
see `scripts/` on the dev box or just `rustup target add
x86_64-unknown-linux-musl` + `apt install musl-tools` if starting fresh).

## Getting a change into a real microVM

Building the binary isn't enough — it has to be baked into a rootfs
image before it does anything:
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

## Verifying a change

Compiling isn't proof it works — this crate specifically needs the full
live-boot verification loop (build → inject into a fresh rootfs copy →
boot via `scripts/boot-test-vm.sh` or the daemon → talk to it over vsock)
described in the root `AGENTS.md`. A change here that only "compiles" has
not been verified.

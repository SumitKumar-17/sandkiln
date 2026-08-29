# AGENTS.md — sandkiln-protocol

Read the root `AGENTS.md` first for project-wide conventions (commit
style, no unverified claims, etc). This file is scoped to this one crate.

## What this crate is

The wire format shared by the host (`sandkiln-vmm`, inside the daemon)
and the guest (`sandkiln-guest-agent`, inside the microVM). Both sides
depend on this crate so they can never silently drift out of sync on the
message shapes or the vsock port number.

Deliberately dependency-light (`serde` + `serde_json`, nothing else) and
has zero knowledge of vsock, HTTP, or Firecracker — it only defines
messages and how to frame them on a byte stream. Keep it that way; if
you're reaching for a networking or process-related dependency here,
that code belongs in `sandkiln-vmm` or `sandkiln-guest-agent` instead.

## Files

- `messages.rs` — `Request`/`Response` enums. Every operation the guest
  agent supports (`Exec`, `ReadFile`, `WriteFile`, `ListDir`) is a
  variant here. Adding an operation means adding a variant here first,
  then implementing it in `sandkiln-guest-agent`'s `handler.rs`, then
  exposing it from `sandkiln-vmm` and the daemon.
- `framing.rs` — length-prefixed message framing (4-byte LE length +
  payload) over anything implementing `Read`/`Write`. Chosen over
  newline-delimited framing specifically so binary file contents in a
  response can never be misinterpreted as a frame boundary.
- `lib.rs` — re-exports, plus `AGENT_PORT` (the fixed vsock port both
  sides agree on) and the `encode_*`/`decode_*` helper functions that
  keep `serde_json` an implementation detail callers don't need to
  depend on directly.

## Changing the protocol

Both `sandkiln-guest-agent` and `sandkiln-vmm`/`sandkiln-daemon` need to
be rebuilt and reinjected/redeployed together after a protocol change —
there's no version negotiation. If you add a `Request`/`Response`
variant, grep both crates for existing `match` statements over these
enums (`handler.rs` in guest-agent, wherever `Response` is matched in
`routes.rs`) — Rust will make the compiler catch missing arms, but only
if you actually rebuild both sides.

## Verifying a change

This crate alone has no live-testable behavior (it's just types + pure
framing logic) — `cargo build -p sandkiln-protocol` and
`cargo clippy -p sandkiln-protocol --all-targets` on the remote dev box
(see root `AGENTS.md` for why local builds don't work) is the bar for a
protocol-only change. A behavioral change needs the full
guest-agent + daemon live-boot verification described in the root doc.

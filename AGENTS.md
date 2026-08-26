# AGENTS.md

Read this before touching the code. It exists so anyone — human or
agent — can pick this project up cold and not repeat mistakes already
made and fixed once.

## What this is

`sandkiln`: a compute primitive for safely running untrusted or
AI-generated code in hardware-isolated Firecracker microVMs. Rust core,
TypeScript SDK. See `README.md` for the pitch and `ROADMAP.md` for the
full feature plan — read the roadmap before starting new work, it's kept
current and explains what's done, what's open, and why.

## Repo layout

```
core/crates/protocol/    wire format shared by host and guest
core/crates/guest-agent/ static musl binary, runs inside the VM
core/crates/vmm/         drives Firecracker + networking (host side)
core/crates/daemon/      axum HTTP API (sandkilnd)
packages/sdk/            sandkiln npm package (TypeScript)
images/                  rootfs/kernel fetch + agent-injection scripts
scripts/                 dev-box setup: tap pool, network bridge, DNS proxy, sync
```

## Where the real work happens

Nothing in this repo can be fully tested locally. KVM, Firecracker, and a
real Linux network stack are required — that lives on a remote dev box,
not wherever this repo is checked out. `scripts/remote.sh sync|run|ssh`
pushes the repo there and runs commands. Read that script before assuming
`cargo build` locally does anything meaningful — it won't; there's no
Rust toolchain expectation locally, only on the remote box.

Every change to `core/` needs, at minimum: sync, `cargo build --workspace`,
`cargo clippy --workspace --all-targets` (zero warnings is the bar, not a
suggestion), then an actual live test — boot a sandbox through the daemon
and hit it. "It compiles" is not "it works" for this project; every
commit in the history that claims a feature works was verified against a
real running daemon and a real microVM first.

## Non-obvious things that will waste your time if you don't know them

- **`setcap` does not survive a rebuild.** The daemon binary loses its
  `CAP_NET_ADMIN` grant every time `cargo build --release` produces a new
  binary. Re-run `scripts/grant-net-admin.sh <binary>` after every rebuild
  before starting the daemon, or every network operation will fail with
  "Operation not permitted."
- **`#[tokio::main]` starts the runtime before your function body runs.**
  If you need to raise a Linux capability into the ambient set (or do
  anything else that must happen before Tokio spawns its worker/blocking
  threads), don't use `#[tokio::main]` — those threads clone credentials
  at spawn time, before your macro-wrapped body executes, and won't see
  anything you raise inside it. `core/crates/daemon/src/main.rs` does
  this correctly: plain `fn main()`, raise first, build and enter the
  runtime second. This one cost a long debugging session — don't
  reintroduce it.
- **Creating a new tap device via `ip tuntap add` needs real root, not
  ambient `CAP_NET_ADMIN`.** The daemon runs unprivileged with the
  capability raised into its ambient set, which is enough for netlink
  operations (attach/detach/up on an *existing* interface, bridge
  creation) but not for the `TUNSETIFF` ioctl that creates a device.
  That's why tap devices are pre-created once via
  `scripts/create-tap-pool.sh` (needs sudo) and the daemon only ever
  leases/releases from that pool. Don't try to make the daemon create
  taps on demand without re-solving this.
- **`pkill -f` can match its own command line and kill the wrong thing** —
  including the SSH session running it, if the search pattern happens to
  appear in the invocation itself. Use `pkill -x <exact-process-name>`
  instead.
- **DNS on the dev box's network is unusual**: direct queries to
  `8.8.8.8`/`1.1.1.1` don't work reliably, but resolution through the
  host's own `systemd-resolved` stub does. `scripts/start-dns-proxy.sh`
  runs a `dnsmasq` forwarder for exactly this reason — don't replace it
  with a "simpler" direct-to-public-resolver setup, it'll intermittently
  break.
- **axum's blanket `IntoResponse` for `()` returns `200` with an empty
  body, not `204`.** If a handler's contract says 204, return
  `StatusCode::NO_CONTENT` explicitly — this exact mismatch broke the SDK
  once (found via live integration testing, not by inspection).

## Conventions

- **No "Phase N" language anywhere** — not in code comments, not in
  commit messages, not in docs. This is a real project, not a tutorial
  series. If you find yourself writing "Phase 4:", rename the concept to
  what it actually is (a subsystem, a milestone, a feature name).
- **Commit small and often.** Each commit should be one coherent,
  buildable, ideally-tested unit of change — not a giant batch. No
  `Co-Authored-By` trailers on commits in this repo.
- **Never mention any commercial platform by name** in code, docs, or the
  website — this project takes feature inspiration from prior art in the
  space but is its own, unaffiliated, self-hosted product.
- **Zero clippy warnings** on every commit touching `core/`. Fix them,
  don't `#[allow]` them away unless you can justify why in a comment.
- **No unused/speculative code.** Don't stub out a client method for a
  server endpoint that doesn't exist yet, don't add a config field
  nothing reads. If the roadmap says a feature is planned, the code
  reflects that it's *not* there yet, not a half-built version of it.

## Verifying a change actually works

The pattern used throughout this project's history:
1. `scripts/remote.sh sync`, build, clippy — zero warnings.
2. Grant `CAP_NET_ADMIN` if the daemon binary was rebuilt.
3. Start `sandkilnd` with real env vars pointing at the real kernel/rootfs
   images and tap pool on the dev box.
4. Drive it with `curl` (or the real SDK, port-forwarded — see
   `git log` for the DELETE-status-code bug this caught) against the
   actual HTTP API, not a mock.
5. Clean up: stop the daemon, kill any leftover `firecracker` processes
   (`pkill -x firecracker`), remove temp rootfs copies.

Skipping step 4 is how bugs like the two above shipped in the first
place — they both compiled and clippy-passed cleanly.

## What's next

Check `ROADMAP.md`'s "What works today" section for current state, and
pick the next unclaimed item from whichever subsystem section is most
load-bearing for what you're trying to build. The networking, auth, tags,
and file-op work all followed the same shape: implement, build clean,
test live on real hardware, commit small, update the roadmap to match
reality.

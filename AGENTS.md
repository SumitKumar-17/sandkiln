# AGENTS.md

Read this before touching the code. It exists so anyone — human or
agent — can pick this project up cold and not repeat mistakes already
made and fixed once.

## What this is

`sandkiln`: a compute primitive for safely running untrusted or
AI-generated code in hardware-isolated Firecracker microVMs. Rust core,
JS/TS + Python SDKs, a CLI. See `README.md` for the pitch and
`ROADMAP.md` for the full feature plan — read the roadmap before
starting new work, it's kept current and explains what's done, what's
open, and why.

## Repo layout

```
core/crates/protocol/    wire format shared by host and guest
core/crates/guest-agent/ static musl binary, runs inside the VM
core/crates/vmm/         drives Firecracker + networking (host side)
core/crates/daemon/      axum HTTP API (sandkilnd)
packages/sdk/            sandkiln npm package (TypeScript)
packages/python/         sandkiln PyPI package (Python)
packages/cli/            kiln CLI, wraps the JS/TS SDK
images/                  rootfs/kernel build + agent-injection scripts
scripts/                 dev-box setup: tap pool, network bridge, DNS proxy, sync
website/                 the project site, deployed via GitHub Pages
```

**Each of these has its own `AGENTS.md`** with details scoped to that
one piece (its specific gotchas, how to build/verify just that part) —
read this file for project-wide context, then the relevant package's own
`AGENTS.md` before working inside it. If you were handed just one
package directory and not this whole repo, that package's `AGENTS.md`
is written to be enough to start from — though skimming this file too,
if you have access to it, still helps.

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
  buildable, ideally-tested unit of change — not a giant batch. There is
  no limit on commit count; more small commits is always preferred over
  fewer large ones.
- **Git identity is fixed: `SumitKumar-17 <sumitkanpur2005@gmail.com>`,
  always.** Never the ambient session email, never any other name. No
  `Co-Authored-By` trailer on any commit in this repo, ever — this is a
  personal project with one author.
- **Never mention any commercial platform by name** in code, docs, or the
  website — this project takes feature inspiration from prior art in the
  space but is its own, unaffiliated, self-hosted product. This extends
  to comparable products researched for feature ideas (see `ROADMAP.md`)
  — take the idea, never the name or branding.
- **Zero clippy warnings** on every commit touching `core/`. Fix them,
  don't `#[allow]` them away unless you can justify why in a comment.
- **No unused/speculative code.** Don't stub out a client method for a
  server endpoint that doesn't exist yet, don't add a config field
  nothing reads. If the roadmap says a feature is planned, the code
  reflects that it's *not* there yet, not a half-built version of it.
- **Every crate needs real unit tests, not just compile-clean code.**
  Colocated `#[cfg(test)] mod tests` at the bottom of the file they test
  is the convention here (idiomatic Rust, keeps a test next to what it
  covers) — not a separate `tests/` tree per file. Prefer pulling pure
  logic out of framework plumbing specifically so it's unit-testable
  (e.g. `auth::token_matches` pulled out of the axum middleware around
  it) over skipping the test because "it needs a real request." Use real
  temp-directory filesystem state instead of mocking wherever the
  operation doesn't need KVM.
- **No comments explaining what code does — only why, and only when it's
  non-obvious.** A hidden constraint, a workaround for a specific bug, an
  invariant a reader could easily violate — those earn a short comment.
  Restating the code in prose does not. If a change needs real design
  explanation (why this architecture, what alternatives were rejected and
  why), that belongs on the website's architecture material, not as a
  comment block in the source.
- **Keep files scoped to one concern; split when a file outgrows it.**
  `routes_drives.rs`/`routes_snapshot.rs`/`routes_sandbox.rs`/
  `routes_exec.rs` and `vm/mod.rs`+`vm/snapshot.rs` are the precedent —
  when a file starts covering two things that could reasonably be worked
  on independently (or reads past ~250-300 lines for no structural
  reason), split along that seam rather than letting it grow. This also
  keeps parallel-agent work from producing merge conflicts on one shared
  giant file.
- **Every package/crate directory gets its own `AGENTS.md`**, kept in
  sync with that directory's actual structure (see the list at the top
  of this file). When you split or rename files, update the relevant
  `AGENTS.md`'s file list in the same commit — a stale map is worse than
  no map.

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

## Working with multiple agents in parallel

When several independent pieces of work are ready at once, prefer running
them as parallel, worktree-isolated agents over one long serial session:
each agent reads this file and the relevant package `AGENTS.md`, works
without KVM access (code changes only — the host verifies live
afterward), commits in small increments, and avoids touching shared docs
(`ROADMAP.md`, root `AGENTS.md`, `CHANGELOG.md`) to keep merge conflicts
rare. Reconcile branches with `cherry-pick`, not merge, when their
history has diverged from a rewritten `main`. After merging, always run
the full verification pass (build, clippy, test, then live-test on the
dev box) — an agent's own report that something "works" is not a
substitute for that.

## Self-hosting

See `SELF_HOSTING.md` for the full guide to standing up your own instance
of this service end to end (host requirements, building the images,
running the daemon, networking setup). Keep it in sync with `scripts/`
and `core/crates/daemon/src/config.rs` — a self-hosting doc that
references a removed script or a renamed env var is worse than none.

## What's next

Check `ROADMAP.md`'s "What works today" section for current state, and
pick the next unclaimed item from whichever subsystem section is most
load-bearing for what you're trying to build. The networking, auth, tags,
and file-op work all followed the same shape: implement, build clean,
test live on real hardware, commit small, update the roadmap to match
reality.

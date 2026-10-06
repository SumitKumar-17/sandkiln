# AGENTS.md

Project-wide engineering standard. Read this first, then the relevant
package's own `AGENTS.md`.

## Project

`sandkiln` runs untrusted or AI-generated code inside hardware-isolated
Firecracker microVMs. Personal project; not an excuse for shortcuts in
correctness, security, maintainability, or developer experience. Read
`ROADMAP.md` before substantial new work — it's current reality plus the
reasoning behind it.

```
core/crates/protocol/    wire format shared by host and guest
core/crates/guest-agent/ static musl binary, runs inside the VM
core/crates/vmm/         drives Firecracker + networking (host side)
core/crates/store/       durable sandbox-lifecycle history (sqlite)
core/crates/daemon/      axum HTTP API (sandkilnd)
packages/sdk/            sandkiln npm package (TypeScript)
packages/python/         sandkiln PyPI package (Python)
packages/cli/            kiln CLI, wraps the JS/TS SDK
images/                  rootfs/kernel build + agent-injection scripts
scripts/                 dev-box bootstrap, daemon lifecycle, tests
website/                 marketing pages + Starlight docs, one Astro project
examples/                runnable reference projects — see examples/AGENTS.md
docs/architecture/       brief, code-referenced system tour (not the website)
```

## Where the real work happens

KVM/Firecracker/real networking can't be faked locally — everything runs
on the remote dev box via `scripts/remote.sh sync|run|ssh`. A local
`cargo build` proves nothing; read `remote.sh` before assuming otherwise.

Fresh box: `scripts/setup.sh` (idempotent bootstrap). After that,
`scripts/sandkilnd-ctl.sh start|stop|restart|status|logs` needs no env
vars — see `SELF_HOSTING.md`'s Quick Start.

---

## 1. Engineering standard

Solve the actual problem, not the smallest diff. Optimize for
correctness, completeness, maintainability, security, performance,
testability, reproducibility, developer experience — not line count or
file count.

Fix root causes, not symptoms (precedent: the DELETE-status-code bug, the
vsock-hang from an unbounded I/O wait, the tap-creation privilege gap —
all root-caused, not patched around).

Refactors, API changes, new libraries/services, framework/build-system
swaps are all in scope when they materially improve the system. Not in
scope: rewriting something that already works, for its own sake.

## 2. Scope calibration

- **Website**: a request for "more functionality" means inspect the real
  architecture and fix the actual limitation, including a rewrite if
  that's the right call. See `website/AGENTS.md`.
- **Performance**: measure the whole path first (`ROADMAP.md`'s
  Benchmarking section). Don't optimize around a bottleneck you haven't
  identified. Precedent: boot latency turned out to be the rootfs clone
  (74%), not the network lease, found only by profiling.
- **Dev tooling**: "write a script" means a reliable entrypoint —
  prereq/dependency checks, config, cleanup, useful failures — not a
  one-line wrapper. See `scripts/integration-test.sh`.
- **File splitting**: split a file once it mixes independently-workable
  concerns, or passes ~250-300 lines for no structural reason
  (`routes_drives.rs`/`routes_snapshot.rs`/`routes_sandbox.rs` are the
  precedent). A large file that's one dense, cohesive invariant
  explanation (not mixed concerns) doesn't need forcing apart — check
  which case you're in before splitting.

## 3. Inspect before editing

Read this file, the package's `AGENTS.md`, relevant `ROADMAP.md`
sections, the existing implementation, its consumers, its tests, and its
scripts/config — in that order — before writing code. Don't guess when
the repo can answer it.

## 4. Trace the whole system

```text
protocol → guest agent → VMM → daemon → HTTP API → JS SDK → Python SDK
  → CLI → website → examples → tests → documentation
```

Update every layer a change actually touches. A new daemon endpoint with
no SDK method, no CLI command, no test, is "started," not "done."

## 5. No half-finished surfaces

No fake implementations, placeholder responses, unused speculative APIs,
SDK methods for endpoints that don't exist, or TODOs presented as done.
A deliberately deferred feature stays marked "planned," not implied done
(`ROADMAP.md`'s done/partial/planned states; the website's feature grid
follows the same rule).

## 6. Testing and verification

Compilation is not proof. Every crate needs real unit tests, not just
compiling code — pull pure logic out of framework plumbing so it's
testable (`auth::token_matches`, `idle_reaper::is_idle`) rather than
skipping the test.

- `cargo test --workspace` (from `core/`) — pure logic, no KVM needed.
- `scripts/integration-test.sh [base-url]` — full daemon API against a
  real `sandkilnd`, 24 topic files under `scripts/integration-tests/`,
  run concurrently by default. A topic must compare exact id sets, never
  a global count (`18-pool.sh` is the pattern) — concurrent topics make
  global counts flaky. Add a case here for every new HTTP-facing feature.
- `scripts/load-test.sh [concurrency] [iterations] [base-url]` —
  concurrency/latency under load.
- `cargo bench -p sandkiln-vmm --bench vm_lifecycle` — boot/exec/snapshot/
  resume timing against real Firecracker. Numbers live in `ROADMAP.md`.
- `scripts/bench-report.sh [iterations]` — fast repeatable regression
  check against `/metrics`, diffed against the last run.
- `scripts/dev.sh <subcommand>` — thin dispatcher over all of the above.

"It compiles" is not "it works" here — every shipped feature was verified
against a real daemon and a real microVM first.

## 7. Failure paths matter

```text
allocate resource → configure filesystem → configure network
  → start VM → connect guest → register sandbox
```

Every intermediate failure must leave the system valid (precedent:
`create_sandbox` releases the network lease if `Vm::boot` fails;
`Vm::stop` syncs before killing; a failed snapshot releases its
resources). Don't implement only the happy path.

## 8. Resource ownership

Every resource (process, file, socket, tap device, lease, VM, snapshot,
image) needs clear ownership: who creates it, who cleans it up, what
happens if the owner crashes. `AppState::drive_holder()` is the
precedent — one place that answers "who holds this" across live
sandboxes and held snapshots, which is what prevents double-attaching a
drive to two VMs.

## 9. Lifecycle correctness

Model valid states and transitions explicitly (e.g.
`Creating → Starting → Running → Stopping → Stopped`, with failure
states) rather than ad-hoc flags and cleanup — applies to sandboxes,
sessions, snapshots, resume/suspend/fork, drives, networking, images.

## 10. Performance

Evidence-driven only: baseline, measure the whole path, find the real
bottleneck, fix it, re-measure. Record before/after numbers in
`ROADMAP.md`'s Benchmarking section (directional, one shared dev box —
not authoritative).

## 11. Security

Security and isolation are core functionality, not a layer on top.
Validate at the server boundary even when a client already does
(precedent: `drive::validate_id` rejects path traversal daemon-side; the
daemon runs unprivileged with only ambient `CAP_NET_ADMIN`; bridge port
isolation blocks sandbox-to-sandbox traffic at the network layer, not
just the API).

## 12. Development environment gotchas

- `setcap` doesn't survive a rebuild — re-run
  `scripts/host-setup/grant-net-admin.sh <binary>` after every build.
- Raise capabilities before entering the Tokio runtime, not inside
  `#[tokio::main]` — its worker threads clone credentials at spawn time.
  `main.rs` does this correctly: plain `fn main()`, raise first.
- Creating a tap device (`ip tuntap add`) needs real root, not ambient
  `CAP_NET_ADMIN` — that only covers netlink ops on an existing
  interface. Tap devices are pre-created once via
  `scripts/host-setup/create-tap-pool.sh`; the daemon only leases them.
- Use `pkill -x <exact-name>`, not `-f` — `-f` can match its own command
  line and kill the wrong thing, including the shell running it.
- The dev box's DNS needs `scripts/host-setup/start-dns-proxy.sh`
  (forwards to the host's `systemd-resolved` stub) — direct queries to
  public resolvers don't work reliably there.
- Prefer real readiness checks over sleeps; keep an unavoidable one short
  and justified.
- `cargo`/current `node` aren't on `PATH` in a non-interactive SSH
  shell — source `~/.cargo/env`/`~/.nvm/nvm.sh` first.
  `scripts/remote.sh run|ssh` already does this.
- A non-interactive shell can't prompt for sudo — run
  `sudo scripts/host-setup/allow-passwordless-cap-grant.sh` once,
  interactively, so a rebuild-then-restart over SSH just works.
- A near-full disk can silently truncate a file mid-write (seen with an
  `npm install` native addon) — a `SIGBUS` crash on a native addon means
  check disk space before assuming a code bug.

## 13. Documentation

Update docs in the same change as the behavior — API, SDK, CLI, config,
networking, security, self-hosting. Keep package `AGENTS.md` file lists
synced when files move. No "Phase N" language anywhere — name the real
concept. Never name a commercial platform in code/docs/website — generic
technique descriptions only (a required filename like `vercel.json` is
not "naming" it). Comments explain *why*, only when non-obvious — not
what the code already says, and not a design essay (that belongs in
`docs/architecture/` or the website, not a source comment).

## 14. Architecture and abstractions

Use an abstraction when it isolates a real boundary or removes real
duplication — not for theoretical future reuse. Don't tolerate
duplication or coupling just because fixing it needs a refactor.

## 15. New libraries and services

Fine when it's a real engineering boundary (ownership, API, lifecycle,
failure behavior, testing strategy all thought through) — not as
architectural decoration.

## 16. Parallel agents

Good split boundaries: website, each SDK, CLI, individual Rust crates,
tests, docs. Define scope and verification up front; avoid concurrent
edits to `ROADMAP.md`/this file/`CHANGELOG.md`. After merging: inspect
every diff, reconcile shared-file conflicts by combining both sides, run
the full verification pass (build, clippy, test, live), update
`ROADMAP.md`/`CHANGELOG.md`. An agent saying "it works" is not
verification — same bar as a human contributor's claim.

## 17. Git

Commit small and often, no cap on commit count. Identity is always
`SumitKumar-17 <sumitkanpur2005@gmail.com>` — never the session default,
never a `Co-Authored-By` trailer.

## 18. Final review

Before calling anything done: check the diff, remove debug code, verify
deps/tests/docs/config/error-handling/cleanup/security, then actually
run Section 6's verification. Compiling is not done.

## 19. Definition of done

Normal path works, invalid input handled, failure paths correct,
resources cleaned up, tests present, affected clients updated, docs
accurate, verified on the real environment. If any of that's missing,
say so — don't claim done.

## 20. Final principle

Make the scope decision on engineering reality, not fear of a large
diff. Small fix → small change. Refactor needed → refactor. New
service needed → build it.

---

## What's next

`ROADMAP.md`'s "What works today" section has current state; pick the
next unclaimed item from whichever section is most load-bearing.

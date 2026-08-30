# AGENTS.md

Read this before touching the repository. This file defines the
project-wide engineering standard. Package-specific `AGENTS.md` files
define additional rules for that package — read this one first, then
the relevant package's.

## Project

`sandkiln` is a serious, long-lived project for safely running untrusted
or AI-generated code inside hardware-isolated Firecracker microVMs.

The repository contains Rust core infrastructure, Firecracker/VMM
integration, a daemon, TypeScript and Python SDKs, a CLI, images,
networking/development tooling, a website, examples, tests, and
benchmarks:

```
core/crates/protocol/    wire format shared by host and guest
core/crates/guest-agent/ static musl binary, runs inside the VM
core/crates/vmm/         drives Firecracker + networking (host side)
core/crates/daemon/      axum HTTP API (sandkilnd)
packages/sdk/            sandkiln npm package (TypeScript)
packages/python/         sandkiln PyPI package (Python)
packages/cli/            kiln CLI, wraps the JS/TS SDK
images/                  rootfs/kernel build + agent-injection scripts
scripts/                 dev-box setup: tap pool, network bridge, DNS proxy, sync, integration/load tests, preflight checks, systemd install
website/                 the project site, deployed via GitHub Pages (and a live mirror)
examples/                runnable reference projects (code playground, agent runner)
```

Personal-project status does **not** justify shortcuts in correctness,
security, maintainability, reproducibility, or developer experience.

Read `ROADMAP.md` before substantial new work. It describes current
reality, planned work, and the engineering reasoning behind it.

## Where the real work happens

Nothing in this repo can be fully tested locally. KVM, Firecracker, and a
real Linux network stack are required — that lives on a remote dev box,
not wherever this repo is checked out. `scripts/remote.sh sync|run|ssh`
pushes the repo there and runs commands. Read that script before assuming
`cargo build` locally does anything meaningful — it won't; there's no
Rust toolchain expectation locally, only on the remote box.

---

# 1. Engineering Standard

## Solve the actual problem

The user's request describes the desired outcome. It does **not** define
the maximum implementation scope.

Do not optimize for:

* smallest diff
* fewest files changed
* fewest lines of code
* fastest implementation
* preserving existing architecture at all costs

Optimize for:

* correctness
* completeness
* maintainability
* security
* performance
* testability
* reproducibility
* developer experience

If the correct solution requires substantial engineering, do the
substantial engineering.

## Fix root causes

Do not hide architectural problems behind local workarounds.

If implementing a feature exposes a deeper problem, fix the deeper
problem when it is relevant to the requested outcome. This project's own
history is full of exactly this: the DELETE-status-code bug, the vsock
hang caused by an unbounded I/O wait, the tap-creation privilege gap —
every one of those was root-caused and fixed properly, not patched around
(see "Non-obvious things" below for the specifics).

## Refactoring and architectural changes are explicitly allowed

Existing architecture is not sacred. When technically justified, you may
refactor large portions of the codebase, split or merge packages, replace
abstractions, change APIs, migrate frameworks, replace build systems,
redesign storage, introduce new libraries or services, or rewrite a
component.

Do this when it materially improves correctness, security,
maintainability, performance, or the ability to implement the requested
feature. Do **not** perform unrelated rewrites merely because existing
code could theoretically be cleaner.

---

# 2. Examples of Appropriate Scope

These are examples of the expected engineering mindset, not restrictions
on implementation.

### Website

If asked to improve or add substantial website functionality: inspect the
complete website architecture, identify the actual limitation, implement
the appropriate solution — including restructuring, changing the build
approach, or rewriting major portions if that's genuinely the best
engineering solution. `website/AGENTS.md` has this page's specific
constraints (single static file, no build step, design-rationale content
belongs here rather than in new root-level `.md` files).

### Performance / optimization

If asked to optimize sandbox startup, first measure the complete path
(see `ROADMAP.md`'s Benchmarking section for the existing methodology and
numbers). If the bottleneck is architectural, do not repeatedly optimize
around it — the rootfs-copy-dominates-boot-latency investigation is the
precedent: measured, found the real bottleneck, fixed it properly
(concurrent lease + `cp --reflink=auto`), left the remaining gap
explicitly documented as needing a CoW filesystem or thin-provisioning
layer rather than another local hack. If the correct solution requires
building an entire library or service, build it — but measure first and
choose the solution that actually addresses the bottleneck.

### Development tooling

If asked to create a dev script, interpret that as "build a reliable
entrypoint," not "put one command inside a shell script." Inspect the
repository and implement whatever is actually required: prerequisite
checks, dependency checks, configuration, builds, networking,
permissions, readiness checks, logging, cleanup, safe repeated
invocation, useful failure messages. `scripts/integration-test.sh` and
`scripts/load-test.sh` are the precedent for what a real verification
script looks like, not a one-line wrapper.

### Splitting files

`routes_drives.rs`/`routes_snapshot.rs`/`routes_sandbox.rs`/
`routes_exec.rs` and `vm/mod.rs`+`vm/snapshot.rs` are the precedent for
when a file covers two things that could reasonably be worked on
independently, or reads past ~250–300 lines for no structural reason:
split along that seam. This also keeps parallel-agent work from producing
merge conflicts on one shared giant file.

---

# 3. Inspect Before Editing

For non-trivial work, do not start coding from the request alone. Before
modifying code:

1. Read this file.
2. Read the relevant package/crate `AGENTS.md`.
3. Read relevant `ROADMAP.md` sections.
4. Inspect the existing implementation.
5. Search for consumers and related functionality.
6. Inspect tests and benchmarks.
7. Inspect scripts/configuration.
8. Inspect relevant documentation.
9. Identify the change's blast radius.
10. Identify how the result will actually be verified.

Do not guess when the repository can answer the question.

---

# 4. Trace the Whole System

Do not stop after modifying the obvious file. For changes that cross
boundaries, inspect all affected layers:

```text
protocol
  ↓
guest agent
  ↓
VMM
  ↓
daemon
  ↓
HTTP API
  ↓
TypeScript SDK
  ↓
Python SDK
  ↓
CLI
  ↓
website
  ↓
examples
  ↓
tests
  ↓
documentation
```

Not every change affects every layer. Determine the actual impact and
update every affected consumer. A feature is not complete when only one
layer supports it — a new daemon endpoint with no SDK method, no CLI
command, no test, and no roadmap update is not "done," it's "started."

---

# 5. No Half-Finished Surfaces

Never present incomplete functionality as finished. Do not add fake
implementations, placeholder success responses, unused speculative APIs,
SDK methods for nonexistent endpoints, configuration that nothing reads,
disabled functionality pretending to work, or TODO implementations
presented as complete.

If a feature is deliberately deferred, leave it explicitly deferred —
`ROADMAP.md` marks things "Planned" vs. "Done" for exactly this reason,
and the website's feature grid follows the same rule (a card says
"Shipped" only if it's actually verified working on real hardware).

---

# 6. Testing and Verification

Compilation is not proof. Tests should cover the behavior that matters —
unit, regression, integration, end-to-end, live, benchmark,
failure-path, and concurrency tests as appropriate. When fixing a bug,
add regression coverage whenever practical. Do not weaken tests merely to
make them pass, remove assertions because they expose a problem, or
replace real behavior with mocks simply to make testing easier.

Concretely, in this repo:

- **`cargo test --workspace`** (from `core/`) — pure logic, colocated
  `#[cfg(test)] mod tests`, no KVM needed. Every crate needs real unit
  tests, not just compile-clean code. Prefer pulling pure logic out of
  framework plumbing specifically so it's unit-testable (e.g.
  `auth::token_matches` pulled out of the axum middleware around it,
  `idle_reaper::is_idle` pulled out of the reaper loop) over skipping the
  test because "it needs a real request." Use real temp-directory
  filesystem state instead of mocking wherever the operation doesn't need
  KVM.
- **`scripts/integration-test.sh [base-url]`** — the full daemon API
  exercised end to end against a real running `sandkilnd`: sandbox
  lifecycle, tag filtering, drives (persistence across sandboxes,
  conflict detection), snapshot/resume, auth, `/metrics`, error cases.
  Tracks and tears down everything it creates on exit, pass or fail. Run
  with `SANDKILN_AUTH_TOKEN` set to also exercise auth-rejection cases.
  **Every new HTTP-facing feature should add a case here** — that's the
  entire point of it existing instead of staying tribal knowledge in a
  chat transcript.
- **`scripts/load-test.sh [concurrency] [iterations] [base-url]`** —
  concurrency/latency under load, min/max/mean/p95 per phase.
- **`cargo bench -p sandkiln-vmm --bench vm_lifecycle`** — boot time and
  exec latency, against the real Firecracker binary. See `ROADMAP.md`'s
  Benchmarking section for the env vars needed and the current numbers.

For behavior involving Firecracker, KVM, networking, or the real daemon,
use the actual environment (the remote dev box) — "it compiles" is not
"it works" for this project; every commit in the history that claims a
feature works was verified against a real running daemon and a real
microVM first.

---

# 7. Failure Paths Matter

Design and test what happens when operations fail halfway through:
partial initialization, cancellation, timeout, process crash, network
failure, filesystem failure, dependency failure, duplicate/concurrent
requests, cleanup, recovery, stale resources.

```text
allocate resource → configure filesystem → configure network
  → start VM → connect guest → register sandbox
```

Every intermediate failure must leave the system in a valid state. The
existing precedent: `create_sandbox` releases the network lease if
`Vm::boot` fails; `Vm::stop` best-effort `sync`s before killing so a
guest crash mid-shutdown doesn't lose drive writes; `snapshot_sandbox`
stops the VM and releases resources on a failed snapshot rather than
leaving a half-paused VM dangling. Do not implement only the happy path.

---

# 8. Resource Ownership

Every resource created by the system should have clear ownership:
processes, files, directories, sockets, ports, TAP devices, network
leases, VMs, snapshots, images, temp files, persistent records. Know who
creates it, who owns it, who cleans it up, what happens if the owner
crashes, how stale resources are recovered.

`AppState::drive_holder()` is the precedent for making ownership
explicit and centrally checkable rather than scattered — it answers "who
currently holds this drive" across both live sandboxes and held
snapshots in one place, which is exactly what prevented a real
data-corruption class of bug (double-attaching one drive to two VMs).

---

# 9. Lifecycle Correctness

For lifecycle-heavy functionality, identify valid states and transitions
(e.g. `Creating → Starting → Running → Stopping → Stopped`, with failure
states) rather than implementing complex lifecycle behavior as unrelated
flags and ad-hoc cleanup. Especially relevant here: sandbox lifecycle,
sessions, snapshots, resume, suspend, VM forking, drives, networking,
image preparation.

---

# 10. Performance

Performance work must be evidence-driven: establish a baseline, measure
the complete path, identify the bottleneck, understand why it's
expensive, choose the appropriate solution, implement it, benchmark
again, verify correctness. Do not reject a large architectural
optimization merely because it's large. Do not introduce a large
optimization without evidence it solves a real problem. For meaningful
performance changes, record the workload and before/after measurements —
`ROADMAP.md`'s Benchmarking section is where those live for this project,
explicitly caveated as directional numbers from one shared dev box, not
authoritative.

---

# 11. Security

Sandkiln executes untrusted and AI-generated code. Security and isolation
are core functionality, not a layer bolted on afterward. Never weaken a
security boundary merely because it makes development easier.

For security-sensitive changes consider: privilege boundaries, filesystem
isolation, networking, capabilities, seccomp, cgroups, user/group
separation, device access, KVM access, authentication, authorization,
resource exhaustion, malicious input, command injection, path traversal,
cleanup after crashes.

Validate security boundaries at the daemon/server boundary even when
clients validate too — the existing precedent: `drive::validate_id`
rejects path traversal before it ever reaches a filesystem call, not just
in a client-side check; the daemon runs unprivileged with only ambient
`CAP_NET_ADMIN` rather than as root specifically to bound the blast
radius of a compromised daemon process (see `SELF_HOSTING.md`'s "Why not
just run as root"); bridge port isolation stops sandbox-to-sandbox
traffic at the network layer, not just at the API layer.

---

# 12. Development Environment

Developer workflows are part of the product. They must be reproducible,
documented, idempotent, safe, observable, and maintainable. Avoid
hard-coded personal paths, hard-coded credentials, unexplained
environment variables, hidden manual steps, broad process killing,
arbitrary sleeps, silent failures. A development command should be safe
to run repeatedly.

This repo has already hit and fixed several of these the hard way — don't
reintroduce them:

- **`setcap` does not survive a rebuild.** The daemon binary loses its
  `CAP_NET_ADMIN` grant every time `cargo build --release` produces a new
  binary. Re-run `scripts/grant-net-admin.sh <binary>` after every
  rebuild before starting the daemon, or every network operation fails
  with "Operation not permitted."
- **`#[tokio::main]` starts the runtime before your function body runs.**
  If you need to raise a Linux capability into the ambient set before
  Tokio spawns its worker/blocking threads, don't use `#[tokio::main]` —
  those threads clone credentials at spawn time, before your
  macro-wrapped body executes. `core/crates/daemon/src/main.rs` does this
  correctly: plain `fn main()`, raise first, build and enter the runtime
  second. This one cost a long debugging session.
- **Creating a new tap device via `ip tuntap add` needs real root, not
  ambient `CAP_NET_ADMIN`.** Ambient `CAP_NET_ADMIN` covers netlink
  operations (attach/detach/up on an *existing* interface, bridge
  creation) but not the `TUNSETIFF` ioctl that creates a device. That's
  why tap devices are pre-created once via `scripts/create-tap-pool.sh`
  (needs sudo) and the daemon only ever leases/releases from that pool.
- **Use exact process names, not pattern matching, for process control.**
  `pkill -f` can match its own command line and kill the wrong thing —
  including the SSH session running it. Use `pkill -x <exact-process-name>`.
- **DNS on the dev box's network is unusual**: direct queries to public
  resolvers don't work reliably, but resolution through the host's own
  `systemd-resolved` stub does. `scripts/start-dns-proxy.sh` forwards to
  that for exactly this reason.
- **Prefer real readiness checks over sleeps.** Where a fixed sleep is
  unavoidable (e.g. waiting on a fresh SSH connection to reflect state
  from a command run over a previous one), keep it short and justified,
  not a blanket `sleep 10`.

---

# 13. Documentation

Documentation is part of the implementation. When changing APIs, SDKs,
CLI, configuration, environment variables, networking, security,
architecture, startup, installation, self-hosting, or development
workflows: inspect and update the relevant documentation. Never leave
documentation describing behavior that no longer exists. Keep
package-level `AGENTS.md` files synchronized with actual package
structure — when you split or rename files, update the relevant
`AGENTS.md`'s file list in the same commit.

Project-specific documentation rules:

- **No "Phase N" language anywhere** — not in code comments, commit
  messages, or docs. This is a real project, not a tutorial series. If
  you find yourself writing "Phase 4:", rename the concept to what it
  actually is (a subsystem, a milestone, a feature name).
- **Never mention any commercial platform by name** in code, docs, or the
  website — this project takes feature inspiration from prior art in the
  space but is its own, unaffiliated, self-hosted product. This extends
  to comparable products researched for feature ideas — take the idea,
  never the name or branding. (A required config filename for a hosting
  integration, like `vercel.json`, is a technical necessity, not
  "mentioning" a platform in content — keep the surrounding prose
  platform-agnostic regardless, see `website/AGENTS.md`.)
- **No comments explaining what code does — only why, and only when it's
  non-obvious.** A hidden constraint, a workaround for a specific bug, an
  invariant a reader could easily violate — those earn a short comment.
  Restating the code in prose does not. Real design explanation (why this
  architecture, what alternatives were rejected and why) belongs on the
  website's design-notes material, not as a comment block in the source.
- `SELF_HOSTING.md` is the canonical end-to-end self-hosting guide — keep
  it in sync with `scripts/` and `core/crates/daemon/src/config.rs`.

---

# 14. Architecture and Abstractions

Use abstractions when they solve a real problem. Good abstractions
isolate a real boundary, reduce meaningful duplication, protect
invariants, improve testability, reduce coupling, or support genuine
multiple implementations. Bad abstractions exist only for theoretical
future reuse, wrap trivial code without benefit, add unnecessary
indirection, or make simple behavior harder to understand.

Do not tolerate obvious duplication or architectural coupling merely
because fixing it requires a refactor. Choose the simplest architecture
that correctly solves the actual problem.

---

# 15. New Libraries and Services

Creating a new library or service is completely acceptable when it
represents a real engineering boundary. Before doing so, understand:
responsibility, public API, ownership, lifecycle, dependencies, failure
behavior, security implications, performance requirements, testing
strategy. Possible examples here: image management, storage, caching,
snapshot management, networking policy, scheduling, telemetry,
synchronization, development infrastructure. Do not create services for
architectural decoration; do create them when they're the appropriate
solution.

---

# 16. Parallel Agents

Use parallel agents when work can be safely separated. Good boundaries in
this repo: website, TypeScript SDK, Python SDK, CLI, individual Rust
crates/subsystems, tests, benchmarks, documentation, development tooling.

Before delegating, define scope (files/subsystems owned), expected
behavior, and verification requirements. Agents work in isolated
worktrees, without KVM access (code changes only — the host verifies
live afterward), commit in small increments under the project's git
identity (see Git below), and avoid touching shared docs (`ROADMAP.md`,
this file, `CHANGELOG.md`) simultaneously to keep merge conflicts rare.

After parallel work:

1. Inspect all diffs.
2. Reconcile contracts (two agents adding to the same shared file, e.g.
   `main.rs`/`config.rs`, produces real but usually easily-reconciled
   merge conflicts — resolve by combining both additions, don't just pick
   one side).
3. Run the full verification pass: build, clippy, `cargo test`, then live
   verification on the dev box (`scripts/integration-test.sh` at minimum
   for anything HTTP-facing).
4. Update `ROADMAP.md`/`CHANGELOG.md` to match reality.

An agent's own statement that something "works" is not a substitute for
repository-level verification — that's true of a human contributor's
claim too.

---

# 17. Git

Commit small and often. Each commit should represent one coherent,
buildable unit of work. Commit size must **not** restrict implementation
scope — a large architectural change may require many coherent commits,
and there is no cap on commit count.

**Git identity is fixed: `SumitKumar-17 <sumitkanpur2005@gmail.com>`,
always** — never the ambient session email, never any other name. No
`Co-Authored-By` trailer on any commit in this repo, ever — this is a
personal project with one author.

---

# 18. Final Review

Before declaring work complete: inspect the final diff, remove debug code
and temporary files, check for accidental changes, check dependencies,
tests, documentation, configuration, error handling, cleanup, and
security implications. Then run the appropriate verification (Section 6).
Never declare success merely because the code compiles.

---

# 19. Definition of Done

A task is complete when the requested capability is actually implemented
and the surrounding system remains coherent. Ask: does the normal path
work? Do invalid inputs behave correctly? Do failure paths behave
correctly? Are resources cleaned up? Is lifecycle behavior correct? Are
relevant tests present? Are relevant performance claims measured? Are
security boundaries preserved? Are affected clients updated? Are
scripts/configuration updated? Is documentation accurate? Has the real
required environment been tested? Is there any known incomplete
implementation being presented as finished?

If important work remains, do not claim the task is complete.

---

# 20. Final Principle

**Do not take shortcuts that compromise the system.** The goal is not to
produce the smallest patch — it's to produce the best reasonable
implementation of the requested outcome. If the right answer is a small
change, make a small change. If the right answer is a refactor, refactor.
If the right answer is a new library or service, build it. If the right
answer is replacing a subsystem or a substantial rewrite, do it. Always
make the scope decision based on engineering reality, not fear of a large
diff.

---

## What's next

Check `ROADMAP.md`'s "What works today" section for current state, and
pick the next unclaimed item from whichever subsystem section is most
load-bearing for what you're trying to build.

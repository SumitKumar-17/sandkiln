# Testing and benchmarking

## Unit tests (`cargo test --workspace`)

Pure logic, colocated `#[cfg(test)] mod tests`, no KVM needed. The project's
convention is to pull pure logic *out* of framework plumbing specifically so
it's unit-testable — `auth::token_matches` pulled out of the axum middleware
around it, `idle_reaper::is_idle` pulled out of the reaper loop — rather than
skipping a test because "it needs a real request."

## Integration tests (`scripts/integration-test.sh`) — real daemon, real Firecracker

Every HTTP-facing feature gets a case here: sandbox lifecycle, drives,
snapshot/resume/fork, named sandboxes, images, rate limiting, auth, `/metrics`,
error cases, and so on — 24 numbered topic files under
`scripts/integration-tests/` (`00-health.sh` through `24-env-vars.sh`), each a
self-contained scenario against a real running `sandkilnd`.

- **Runs concurrently by default** (`SANDKILN_INTEGRATION_TEST_PARALLELISM`,
  default 4) — each topic file executes as its own subshell with private
  `WORKDIR`/`PASS`/`FAIL`/`CREATED_*` state, cutting a ~4m10s full run to
  ~70–90s.
- **The concurrency-safety rule this forces**: a topic must never assert on a
  *global* count across the whole daemon (e.g. total snapshot count before/after
  an operation) — a concurrent topic's own churn makes that flaky. Compare
  exact id sets instead (`comm -23` on sorted id lists is the pattern
  `18-pool.sh` uses) — this is a real bug this project hit and fixed, not a
  hypothetical.
- Tracks and tears down everything it creates on exit, pass or fail. Run with
  `SANDKILN_AUTH_TOKEN` set to also exercise auth-rejection paths.

## Load testing (`scripts/load-test.sh`)

Concurrency/latency under load — min/max/mean/p95 per phase, against the same
real daemon, for measuring how the system behaves under concurrent sandbox
launches rather than one at a time.

## Benchmarks (`cargo bench -p sandkiln-vmm`, `scripts/bench-report.sh`)

- `criterion` benchmarks: boot time, exec latency, snapshot-take,
  resume-from-snapshot — against the real Firecracker binary, not mocked.
- `scripts/bench-report.sh`: the fast, routine complement — drives N sequential
  cold creates against a running daemon, diffs `/metrics`' own histograms
  before/after, prints a phase-by-phase breakdown (rootfs clone / network lease
  / boot / total), saves each run for automatic before/after comparison, and
  flags anything that moved more than 10% either way. Built specifically so a
  regression (or an improvement) shows up on the *next* run automatically,
  rather than needing someone to notice it by chance.

## The standing rule behind all of it

"It compiles" has never been treated as "it works" in this project's history —
every feature in `ROADMAP.md`'s "What works today" was verified against a real
running daemon and a real microVM on the actual dev box (KVM/Firecracker/a real
Linux network stack can't be meaningfully faked locally) before being called
done.

## Status

All of the above: done, actively used, and is the actual mechanism behind every
"live-verified" claim in the rest of this `docs/architecture/` folder.

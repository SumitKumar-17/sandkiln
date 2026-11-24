# Redevelopment audit findings

Audit of 2026-11-23, resuming after a long gap. State verified on the dev
box (Intel i9-13900K, 32 threads, 62 GiB, ext4 only, kernel 6.8,
Firecracker v1.16.1), not assumed.

Baseline at the start: `cargo test --workspace` 303 passing, clippy clean.
Code size: daemon 9.4k lines, vmm 3.2k, guest-agent 0.7k, protocol 0.7k,
store 0.4k. End of the audit (2026-11-24): findings 1-19 and 21-24 fixed,
20 open. Live suite 334 of 335 and 333 of 335 on the last two full runs
(the misses: the finding 20 pool flake and a timing bound under 4-way load,
since loosened). 345 unit tests, clippy clean.

At the start the box had rebooted since the last session, so the bridge, tap
pool and DNS proxy were gone (preflight failed only on the missing tap pool,
which needs real root) and the live integration suite had not been re-run;
everything previously claimed as working was as of the last live run, not
re-proven.

Status key: Fixed, Open, Note.

Findings are grouped by theme below, not in number order. Numbers 1-24 are
stable references used across `docs/` and `ROADMAP.md`.

## Dependencies and build hygiene

### 1. Advisories in dependencies. Fixed
- Symptom: RUSTSEC-2026-0459 (bundled SQLite in `rusqlite 0.32`), plus
  unsound-API advisories on `tokio` 1.53.1 and `rustix`.
- Fix: rusqlite 0.43, tokio 1.54, rustix bumped. Tests pass. Re-run
  `cargo audit` in CI.

### 2. `Cargo.lock` was gitignored. Fixed
- Symptom: the workspace ships binaries, so builds were not reproducible.
- Fix: lockfile committed.

### 3. `remote.sh sync` copied local credentials to the shared box. Fixed
- Root cause: the sync tarred the whole tree and `password.txt` was not
  gitignored.
- Fix: excluded from sync and gitignored.
- Remaining chore: a stale `pass.txt` plaintext file sits in the remote
  repo dir. Delete it and rotate the password.

### 7. JS tooling lags (typescript 5.9 vs 7, tsup 8 vs 9). Open, low
Low risk; bump when convenient.

### 18. `remote.sh sync` never deleted removed files. Fixed
- Symptom: a refactor left duplicate modules on the box.
- Fix: pruned rsync over code directories.

## Performance

### 4. Rootfs clone could never reflink. Fixed and measured
- Symptom: 74% of a cold create was the rootfs copy, and whether a clone
  reflinked was invisible.
- Root cause: `clone_rootfs` always copied into `temp_dir()`, so
  `cp --reflink=auto` crossed filesystems and silently fell back to a full
  copy.
- Fix: native `FICLONE` clone, configurable `SANDKILN_ROOTFS_DIR`,
  `rootfs_clone_total{method}` metric.
- Result: on an XFS reflink volume cold create went 187ms to 17.9ms (clone
  127.9ms to 0.08ms, 31 reflinks, 0 copies). This confirmed the earlier
  hypothesis (see [benchmarks](benchmarks.md)).
- Still to decide: make reflink the default for production setups.

### 5. Snapshot-take is 310-990ms. Open
Full memory and disk are written each time. Candidates: Diff snapshots,
dirty-tracked rootfs. See [snapshot cost](design-notes/latency-investigation.md).

### 13. Snapshot requests took 6.8s. Fixed
- Root cause: `snapshot_and_stop` called `Vm::stop()` on a paused VM, whose
  pre-kill `sync` can never be answered. The criterion bench excluded the
  call, hiding it.
- Fix: `force_stop()`. 6.8s to 0.56s (12x).

### 22. Time to first command was 413ms but the site headlined 12.6ms. Fixed
- Root cause: `initcall_debug` showed `loop_init` at 124ms (about 60% of
  guest kernel boot), and the emulated serial console made kernel logging
  expensive.
- Fix: default guest args `quiet loop.max_loop=1` plus a `kernel_args`
  setting.
- Result: create to first exec 441ms to 237ms in the matrix, 413ms to
  214.5ms in bench-report (-48%). Resumed first exec 18ms to 8.4ms.
  Remaining floor is about 125ms of systemd userspace.

## Correctness and data safety

### 9. Snapshots documented as durable lived in the OS temp dir. Fixed
Fix: `SANDKILN_STATE_DIR`.

### 11. Commands silent for more than 3s errored and ran twice. Fixed
- Symptom: `echo run >> f; sleep 4` left `run` twice; the call errored
  after about 6.5s.
- Root cause: the 3s control-plane vsock timeout applied to user commands,
  and `Vm::call` retried after the request had already been delivered.
- Fix: `CallError{request_sent}` so only unsent requests retry, and
  `exec_timeout_secs` (default 600) returning 504.
- Regression: integration topic 25.

### 14. Create path leaked on failure. Fixed
Lease leaked if the clone failed; rootfs file leaked if the lease, boot or
egress failed (AGENTS.md section 7). Fix: both-halves unwind plus the
`OwnedFile` scope guard.

### 16. No graceful shutdown. Fixed
- Symptom: SIGTERM left every Firecracker process running (leaked memory
  and taps) and lost the sandboxes.
- Fix: SIGTERM snapshots every live sandbox (0.75s for 3), 0 processes
  leaked, resumable after restart. Test: `scripts/restart-test.sh`.

### 21. CRITICAL: forks, retain-history resumes and restores wrote into the snapshot's own disk. Fixed
- Root cause: a snapshot records its rootfs by host path, so a VM resumed
  without a drive override wrote straight into it. Full account, including
  the reproduction (a fork's marker on the snapshot's `.ext4` via `debugfs`,
  then ext4 `Bad message` on the next fork), the false-clean first test (the
  second guest's cached metadata masked the damage; dropping caches exposed
  it) and the diagram, is on the
  [snapshot internals page](../docs-site/internals/snapshot-resume-fork.mdx)
  and in engineering notebook post 27. Time-travel restore had the inverse
  flaw (a clone made after resume, given to the record, never used by the
  VM).
- Fix: `ResumeConfig.rootfs_override` (load paused, `PATCH /drives/rootfs`,
  resume). Fork, retain-history resume and restore clone BEFORE resuming.
- Regression: integration topic 26 (13 checks).

### 24. Retained checkpoints filled the disk. Fixed
- Symptom: every resume retained a full-RAM checkpoint plus a rootfs
  forever; ordinary use filled the 468GB dev disk, and snapshots then failed
  with opaque errors.
- Fix: `retained_checkpoints_max` (default 8), oldest evicted after each
  resume; the policy is unit-tested. 73/73 resumes clean afterwards.

## Concurrency

### 12. One slow command froze every sandbox operation. Fixed
- Root cause: `call_agent` held the daemon-wide `sandboxes` mutex for the
  whole vsock call.
- Fix: `AgentEndpoint` copied out, lock released first. `GET /sandboxes`
  answers in 7ms during a running command.

### 23. Pool-claim health check and tunnel start/stop held the same lock. Fixed
- Symptom: a crashed resumed guest froze every request for up to 5s
  (measured 4.9s).
- Fix: `AppState::agent_endpoint`, lock released first.

## Security and configuration

### 15. Bearer token compared with `==`. Fixed
Fix: constant-time comparison.

### 17. Ad hoc configuration. Fixed
30 env vars read with `panic!`/`expect`, defaults duplicated in
`preflight-check.sh`, no config file. Fix: one settings registry, a
`sandkiln.toml` layer, `--check-config`/`--print-config`/`--config-reference`,
and scripts that read the daemon's effective config.

## Code structure

### 6. Large files mixing concerns. Open
`routes_sandbox.rs` 946, `routes_snapshot.rs` 931, `snapshot.rs` 897,
`state.rs` 834 lines. Split per AGENTS.md section 2.

### 19. Daemon `src/` was 36 flat files. Fixed
`main.rs` held the whole router. Now `http/routes`, `lifecycle`,
`snapshots`, `config` module directories.

## Open investigations

### 10. `18-pool` flake. Open
The check "deleting the pool cleaned up its own warm snapshot" failed once
under full-suite concurrency (4/4 in isolation); post-resume
`vsock ... Connection refused` seen in pool-claim health checks. Likely a
resume-readiness race. Probably the same family as finding 20.

### 20. Rare post-resume guest kernel panic. OPEN
- Symptom: in the full 4-way-parallel integration suite, 2-3 of about 320
  checks fail because a freshly resumed guest kernel panics
  (`Attempted to kill the idle task!`, about 20ms after restore) and its
  Firecracker exits.
- Evidence: 160/160 resume and re-snapshot cycles were clean in isolated and
  4-way-parallel loops, and every topic subset passes, so a specific
  concurrent condition is needed. Same family as the pool-claim
  health-check fallback (an earlier console log showed an early-boot
  divide-by-zero trap in the console driver, plausibly TSC or clock-source
  drift between snapshot and restore; unconfirmed).
- Root cause: not found. Finding 21's disk bug was a separate cause and may
  have contributed to old resume failures.
- Next: capture a failing snapshot's `state.snap`/`mem.bin` and the host
  load at the time; test with a CPU template; test other Firecracker and
  kernel versions.

### 8. Stale copies on the remote box. Note
The remote box holds stale copies (`docs/architecture`, worktrees).
Production data to preserve: `~/sandkiln-tools/history.db`, images,
registered images. Do not delete.

## Audit plan, re-measure and resume record

The original plan, in order:
1. Re-establish the live environment (tap pool, DNS proxy, grant caps), run
   the full integration suite, record the real baseline numbers.
2. Fix finding 4 (configurable clone destination plus reflink-vs-copy
   reporting), then re-measure with an XFS or btrfs loopback.
3. Snapshot cost: measure sandkiln's overhead, then evaluate Diff snapshots.
4. Split the oversized daemon files; add admin tooling (CLI status/doctor,
   dashboard of sandboxes, pools, drives, history).
5. Website, docs, packaging and launch pass once the core is re-verified.

Re-measured 2026-11-23 (criterion, this box, kernel 6.8, directional, one
shared machine; previously published figures in parentheses): cold boot
12.6-14.0ms (10.5-10.9), exec round trip 242-248us (220-250), snapshot take
485-727ms (309.6-335.0), resume 3.7-4.0ms (6.5-7.7). Cold `POST /sandboxes`
mean over 20: 187.5ms on ext4 with a full-copy clone, 18.4ms with a
reflinked rootfs. The snapshot HTTP request: 6.8s before finding 13, 0.56s
after. Full table in [benchmarks](benchmarks.md).

State at the end of the session (2026-11-24): findings 1-19 and 21-24 fixed,
20 open; live suite 334 of 335 and 333 of 335 on the last two full runs;
345 unit tests, clippy clean. The "resume here" list that followed is in the
[roadmap](../ROADMAP.md) "What's next". Items with detail not repeated
there:
- Finding 20 root-causing: also note that finding 21's disk bug was a
  separate cause and may have contributed to old resume failures.
- Reflink by default: preflight now warns when the base image and
  `rootfs_dir` do not share a reflink filesystem. `sandkiln.toml` on the box
  points at the XFS loop volume.
- Website: the hero shows "ready for first command" 214ms and "create
  returns" 16.4ms.
- Housekeeping: the box password was used from `password.txt` during this
  work, hence the rotation.

### State of "what works" before the audit

For history: the pre-audit claim was a live suite of 294 checks, 0 failing,
with `SANDKILN_AUTH_TOKEN` set (the script's usage comment lists what is
skipped without one), boot "about 11ms" (it had been about 32ms until a
fixed 20ms socket-wait sleep was found, see
[benchmarks](benchmarks.md)), and `POST /sandboxes` averaging about 144ms
(168ms before the same fix). Several features were then daemon-API-only:
interactive PTY was JS/TS and CLI only, idle-lifecycle archiving was
operator config only, and snapshot lineage and time-travel restore had no
SDK or CLI surface.

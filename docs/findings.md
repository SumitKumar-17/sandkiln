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
  Remaining floor is about 125ms of systemd userspace, not to be confused with Firecracker's 125ms init bound.

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

### 6. Large files mixing concerns. Fixed
The sandbox and snapshot routes each mixed HTTP handlers with the lifecycle
operations, and `held.rs` and `state.rs` each mixed several concerns. The
operations moved to `snapshots/ops/` (take, resume, fork, delete, archive) and
`lifecycle/` (`create`, `create_options`, `stop`); the routes keep DTOs and thin
handlers. `held.rs` split into `held`, `held/reconcile` and a shared
`snapshots/layout`; `state.rs` into `state`, `state/holders` and `state/naming`.

### 19. Daemon `src/` was 36 flat files. Fixed
`main.rs` held the whole router. Now `http/routes`, `lifecycle`,
`snapshots`, `config` module directories.

## Open investigations

### 10. `18-pool` flake. Open
The check "deleting the pool cleaned up its own warm snapshot" failed once
under full-suite concurrency (4/4 in isolation); post-resume
`vsock ... Connection refused` seen in pool-claim health checks. Likely a
resume-readiness race. Probably the same family as finding 20.

### 20. Post-resume guest crash after an early snapshot. FIXED
- Symptom: a resumed guest died about 0.55s after resume (Firecracker exited,
  the vsock call got `Connection refused` for its whole 5s retry window), so
  pool claims fell back to a cold create and `18-pool` flaked.
- Evidence: every failing resume logged `KVM_KVMCLOCK_CTRL call failed` at
  snapshot load; no healthy resume did. With 6 parallel snapshot-then-resume
  loops, a snapshot taken the instant `POST /sandboxes` returned killed 25 of
  55 resumes (45-53% across runs). The same loop with 0.1s, 0.2s, 0.3s, 0.4s,
  0.7s or 1s of delay first killed 0 of 60 per round; 3 rounds of the fixed
  build with no delay killed 0 of 180.
- Root cause: the guest was paused while still settling right after its first
  command; the snapshot then lacks the guest clock state Firecracker re-arms
  on load. The earlier "kernel panic" attribution and the TSC guess were wrong
  about the mechanism: there was no panic text, the VMM simply exited.
- Fix: `Vm::pause` waits until the guest has been running 500ms
  (`MIN_GUEST_AGE_BEFORE_PAUSE`); only a snapshot taken right after create
  waits, by at most a few hundred ms. Full suite went from 343/346 to 345/346,
  the remaining miss being finding 25.
- Not found: the exact guest-side state. If it recurs, capture a failing
  `state.snap` and compare against a healthy one.

### 25. MMDS refresh after resume always failed. FIXED
`Vm::update_metadata` re-applied `PUT /mmds/config` after every pool claim;
Firecracker rejects that on a started VM ("not supported after starting the
microVM"), so claimed sandboxes kept the pool's placeholder metadata while an
old comment blamed a load-related race and a retry loop hid it. It now writes
only `PUT /mmds` (config falls back only if that is refused). `18-pool` passed
3 of 3 runs afterwards.

### 26. Per-VM tunnel listener leaked a thread and a file descriptor. FIXED
`tunnel::listen` spawned an accept loop per VM that never ended, and `cleanup`
only deleted the socket file. After a few hundred VMs the daemon hit its
1024-descriptor soft limit ("Too many open files") with zero sandboxes live.
`cleanup` now flags the loop and wakes it; a unit test proves the thread exits.
The daemon also raises its open-file soft limit to the hard limit at startup.

### 28. Clones of one snapshot shared entropy and a stale clock. FIXED
A restored guest wakes with the random pool and wall clock from snapshot time,
and the 5.10 guest kernel has no automatic reseed after restore (found by the
latency research, [latency-floor](design-notes/latency-floor.md)). Two forks
could generate the same random stream, and a sandbox resumed hours later saw
old time. The daemon now sends `Request::ResumeFixup` after every resume,
claim and fork: the guest agent adds 32 bytes of host entropy
(`RNDADDENTROPY`), forces a reseed (`RNDRESEEDCRNG`) and sets the clock
(`clock_settime`). Best effort: an older agent that does not know the request
only logs a warning, so images must be re-injected with the new agent. Topic
`28-resume-fixup` verifies the clock and the random bytes. The pre-fix
behaviour was not captured, so the topic is evidence for the fix working, not a
recorded before/after.

### 29. Held snapshots pinned a tap device each. FIXED
A snapshot kept its whole network lease, tap included, and `DELETE
/sandboxes/:id` keeps a snapshot by default, so with a 32-tap pool the 33rd
snapshot made every create fail with "no free tap devices" (found by a stress
test). Only the guest's IP and MAC are frozen in the memory image; the tap is a
host-side port, and Firecracker v1.16.1 can repoint a restored interface
(`network_overrides: [{iface_id, host_dev_name}]` on `PUT /snapshot/load`,
confirmed in its API spec and live). The lease is now two: an `IpLease`
(address, MAC, host octet; owned by a running sandbox or a held snapshot) and a
`TapLease` (owned only by a running VM). Snapshot, fork, resume, pool claim and
checkpoint restore all leased a tap at resume time and pass it as the override;
stop and snapshot return it. Consequences handled: egress chains are keyed by
tap, so they are removed when a snapshot is taken and applied to the new tap on
resume; the per-tap "pending restore" set is gone, because a checkpoint
restore now claims its IP atomically from the free pool (`try_reserve_ip`),
which still lets exactly one of two sibling checkpoints restore; a saved tap
name in old `meta.json` files is ignored, so a restart with a smaller or
renamed pool works. A drained pool is now `503`, and `/status` reports tap
usage, IP usage and held-snapshot counts with early warnings. Held snapshots
are now bounded by the /24 (253 addresses), not the tap pool.

Verified live on a 32-tap pool: 40 snapshots created, all succeeded, 0 taps
used; 8 resumed concurrently on distinct taps with working gateway traffic; a
`deny_all` sandbox resumed onto a different tap (and a fork on a third) still
could not reach an address outside the bridge that a control sandbox reached;
daemon restart reconciled all snapshots and they resumed. Topic
`29-snapshots-hold-no-taps` covers it (38 snapshots on a 32-tap pool). Resume
now also attaches a tap (about 4ms of `ip`/`bridge` calls): end to end resume
measured 13.6 and 14.4ms median against 13.1ms recorded before, within the
noise of a shared box.

### 30. The replenisher consumed its own pool. FIXED
Pool replenishment booted its warm instances through the normal create path.
That request has the pool's own shape, so it matched the pool and resumed the
pool's warm snapshot instead of cold booting: a pool wanting 2 warm instances
could never get past 1 (a pool of 1 worked, which hid it). Found when the
default pool, which wants 2, stuck at 1 ready with a growing count of "warm
claims" nobody had made. The replenisher now cold boots directly
(`create_warm_instance`). Regression check in topic 18 (a pool with
`warm_count` 2 reaches 2).

### 31. Pool claims retained the pool's placeholder as a time-travel checkpoint. FIXED
A claim resumed the warm snapshot with history retained, so every pool-backed
create cloned the placeholder's disk, moved it into the retired checkpoints and
counted it against `retained_checkpoints_max`, evicting users' real
checkpoints. It also cost about 50-90ms per create (a warm create measured 78
to 117ms, against 26 to 30ms once consumed). A claim now consumes the warm
snapshot, and the new sandbox starts its own lineage (no dangling parent to the
consumed snapshot).

### 32. The jailer path cannot create a sandbox: the daemon cannot reach the jailed Firecracker's API socket. FIXED
Found while validating the init-mode image with the jailer enabled (setuid
`jailer` copy, cgroup v2, `jailer_enabled = true`, uid range 600000+). Every
`POST /sandboxes` hung until the client gave up (7 minutes and counting; the
create has no deadline on this path). Evidence: three `firecracker`
processes ran as uids 600000-600002 and answered over `api.sock` as root, but
their state was `Not started` with the default `/machine-config`, so none of
the daemon's configuration calls ever arrived. The chroot root jailer creates
is `drwx------` owned by the jailed uid, so the unprivileged daemon cannot
traverse it to `api.sock` (`curl --unix-socket` as the daemon's user fails). The
pre-emptive `o+x` on the chroot root in `prepare_chroot_dir` does not survive
jailer's own ownership setup (the observed mode is 0700). A second gap would follow once that is solved:
the tap pool is owned by the daemon's user, and a jailed Firecracker running
as another uid cannot attach to a tap it does not own. The topic `10-jailer`
therefore cannot pass on any image, init mode or not; "unit-tested but not
verified" in the jailer docs is now "verified not working". Not fixed here: it
needs a design (who owns the chroot and the taps, how the daemon reaches the
socket), not a patch. A hung create also made a graceful daemon stop wait
out its full 30 seconds.

First mitigated (bounded failure, startup self-check), then fixed. Root cause of the hang, found by reproducing it: the
connect to `api.sock` was already bounded (2 seconds), but the failure
cleanup then did `kill()` and an unbounded `wait()` on a Firecracker running
as another uid. The unprivileged daemon's SIGKILL is refused with `EPERM`, so
the wait never returned. Now: the jailed boot waits at most 10 seconds for the
socket (and fails at once if the jailer process dies), every API call has an
I/O deadline, teardown waits at most 5 seconds for a killed process, and a
create fails in about 15 seconds with a message naming the cause. A process
that cannot be killed keeps its uid out of the pool (`JailerIdPool::quarantine`)
rather than handing it to the next VM. At startup the daemon now runs the real
jailer once against a throwaway jail and refuses to start when it cannot enter
the result (`jailer_self_check`, on by default), which is the exact condition
above. Verified on the dev box: startup refused with the finding named, and
with the check disabled a create failed in 15.1 seconds with the explanation,
the lease and rootfs released. Residual: a failed jailed boot leaves a live
process and a root-owned jail directory that only root can remove (the error
and a warning name both).

The fix (`docs/design-notes/security-hardening.md` has the options weighed):
jailer's exec file is now `sandkiln-firecracker-supervisor`
(`core/crates/jail-supervisor`), which runs in the chroot as the VM's uid and a
shared jail group, opens the jail to that group, runs Firecracker, and kills it
when the daemon closes a pipe (the daemon cannot signal another uid, and a VM
no longer outlives a crashed daemon). The daemon's user is in the jail group,
the tap pool is group-owned (`create-tap-pool.sh ... <group>`), the daemon owns
the parent cgroup so it can remove each VM's cgroup, and
`scripts/host-setup/prepare-jailer.sh` does the root steps. The daemon gained
no privilege. Verified on the dev box: a jailed create in about 70ms, exec and
networking work, two concurrent sandboxes, stop removes the jail and cgroup,
a failed boot (an invalid kernel) leaves no process, jail, cgroup, tap or
rootfs, `kill -9` of the daemon ends the jailed Firecrackers, and the topic
`10-jailer` passes. A side effect found on the way: the writable rootfs was
made world-writable for the jailed uid; it now gets the jail group instead.

### 33. A guest that shuts itself down left its sandbox listed. FIXED
`reboot -f` in a systemd guest, or `kill -TERM 1` in an init-mode guest, ends
the Firecracker process, but the daemon kept the sandbox in `GET /sandboxes`
with a dead VM: every call returned `500 Connection refused` until a `DELETE`.
A new watcher (`lifecycle/vm_watcher.rs`) checks each sandbox's VM process once
a second without blocking (`Vm::exit_status`) and tears a dead one down like a
destroy: tap and address back to the pool, egress chain and rootfs removed,
pool slot released, history row ended as `vm_exited`. The teardown skips the
guest `sync` (a dead VM has nothing to flush; it had cost a 5 s wait). Calls to
the gone sandbox answer 404. `/metrics` gains `sandbox_vm_exits_total`.
Integration topic `33-vm-exit-cleanup` (passed 3 of 3, and the full suite
442 of 442) makes a real guest shut itself down and checks all of it.

### 27. Pool claim bookkeeping under full-suite load. FIXED (test isolation)
Intermittent `18-pool` failures: `warm_ready` read 0 where 1 was expected,
the pool's warm snapshot was missing at delete, and `claimed` read 1 with both
of the topic's sandboxes stopped. Not a daemon bug. A pool matches any create
with the same image, vcpu and memory, and every other topic in the concurrent
suite creates default-shaped sandboxes, so they claimed the topic's warm
snapshot and filled its `max_count` slot (the daemon log showed many "claimed
a pre-warmed pool instance" lines during the run). Isolated runs passed 8 of 8
before and after the tap change, which fits. Fix: both pools in the topic now
use a memory size no other topic uses (192 and 160 MiB) and the topic's own
creates carry it; topic 29 already used a non-default shape. Three full runs
afterwards: 393/393 each. Product note: by design a pool serves every
matching create, so on a shared daemon a pool's shape should be chosen to be
the shape you want served.

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

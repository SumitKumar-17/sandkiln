# Feature plan

The ordered plan behind the `ROADMAP.md` "what's next" list. Two goals pull in
parallel and every item is tagged with the one it serves:

- **Latency**: how fast a sandbox is ready (see [latency-floor.md](latency-floor.md), and [benchmarks.md](../benchmarks.md)).
- **Use**: what a user can do with a sandbox (features, integrations, reliability).

Each item names how it is verified. Nothing is "done" until it has been run
against a real daemon and microVM and, where it moves a number, measured with
the [benchmark suite](../../examples/benchmark-suite/README.md).

## Now (in flight)

| Item | Goal | State |
| --- | --- | --- |
| Reproducible benchmark suite as an example | Latency | agent building `examples/benchmark-suite/` |
| MCP server for AI agents (`packages/mcp`) | Use | agent building |
| GitHub Actions action and example | Use | agent building `.github/actions/sandkiln-run`, `examples/github-actions/` |
| Latency floor research | Latency | agent writing `latency-floor.md` |
| Daemon layering refactor | Maintainability | agent moving lifecycle code out of route files |
| `kiln doctor`, `GET /status` | Use | shipped |

## Next, in priority order

### 1. Per-clone fixups after resume (Correctness, shipped; unblocks item 2)
The latency research found that a resumed guest reseeds neither its entropy pool nor its clock (guest kernel 5.10 has no vmgenid reseed), so clones of one snapshot are not unique and a resumed clock is stale. Add a fixup request to the guest protocol that the daemon sends after every resume, claim and fork: reseed the RNG with host entropy, set the wall clock from the host, refresh hostname and id.
Shipped as `Request::ResumeFixup`; verified by integration topic `28-resume-fixup` (clone clock within 2s of the host after a 5s pause, two forks read different random bytes and kernel UUIDs).

### 2. Warm path by default: a user-visible create is a resume (Latency, shipped)
Shipped as `default_warm_count` (default 2): an implicit pool `default` of the daemon's default shape, with warm hits and misses in `/metrics` (`pool_claims_total{result}`) and `/status` (`pool_claims`), and replenishment that pauses when guest addresses or disk run low. Measured paced: create to first command 233.9ms to 26.4ms. See [benchmarks](../benchmarks.md#warm-pool-by-default-2026-11-24) and findings 30 and 31. Remaining: a warm instance for non-default shapes (per-image defaults) and wake-on-claim replenishment, since the replenisher only ticks every 2s.

### 3. Snapshots must not hold a tap device each (Use and scale, shipped)
A held snapshot keeps its tap lease, and `DELETE` keeps a snapshot by default, so 32 snapshots exhaust a 32-tap pool and every create fails (found while stress-testing). The guest only needs a tap at resume. Firecracker can repoint a restored network interface (`network_overrides` on snapshot load), so release the tap at snapshot time and lease any free one at resume; keep the guest IP and MAC frozen. Surface held-snapshot tap usage in `/status` and warn before exhaustion.
Shipped: the lease is split into an `IpLease` (kept by a snapshot) and a `TapLease` (held only while a VM runs), resume repoints the interface with `network_overrides`, and `/status` reports `tap_pool.in_use`, `ip_pool` and warnings. Held snapshots are now bounded by the sandbox /24 (253 addresses) and disk, not by taps. Verified: 40 snapshots on a 32-tap pool, concurrent resumes, deny_all egress after a tap change, restart reconcile, topic `29-snapshots-hold-no-taps` (see finding 29). Not done: 100 snapshots (the /24 allows 253 but each full-RAM checkpoint costs its memory size on disk), and widening the /24 or giving a snapshot an IP only on demand.

### 4. Cold path: custom minimal init and trimmed kernel (Latency, high)
About 125ms of the 214ms is guest systemd userspace (not to be confused with Firecracker's 125ms init bound). Replace it with the guest agent as PID 1 (mounts, network from kernel args, then serve), drop unneeded kernel features, and signal readiness over vsock instead of polling. Target and levers are in the research note.
Verify: benchmark suite time-to-first-command; the full integration suite on both images.

### 5. Snapshot-take cost (Latency, medium)
Taking a snapshot writes the full memory file (310-727ms). Measure sandkiln's own overhead against Firecracker's, then evaluate diff snapshots and writing the memory file to a faster location. Matters for pools (replenish rate) and checkpoint-heavy agent loops.
Verify: benchmark suite snapshot latency; a resume from a diff-chain is bit-identical in a filesystem check.

### 6. Per-request exec timeout that kills the process (Use, shipped)
Shipped: `POST /sandboxes/:id/exec` takes `timeout_secs` (1 to `exec_timeout_secs`, which is also the default); the guest agent runs the command in its own process group, drains both pipes concurrently, and at the limit sends the group SIGTERM then SIGKILL after a 2 second grace, replying `Response::TimedOut` with the output so far, which the daemon returns as a 504 with `timed_out: true`. The no-limit wire shape is unchanged, and an agent that predates the field ignores it and runs unbounded. JS (`timeoutSeconds`, `SandkilnExecTimeoutError`), Python (`timeout=`, sync and async), `kiln sandbox exec --timeout` (exit 124) and MCP `sandbox_exec` use it. Verified live: topic `30-exec-timeout` (a `sleep 30` with `timeout_secs=2` answers in 2.0s and no `sleep` is left in the guest, including grandchildren of a shell), the CLI, both Python clients, the JS SDK, the MCP live smoke and the Action. Not done: `exec-stream` has no timeout (the Action wraps the job in `timeout` inside the guest); a process that calls `setsid` leaves the group and survives the kill (its pipe is abandoned after 2 seconds); a guest agent that predates the field still needs re-injecting before the limit takes effect.

### 7. Pool claim accounting under load, finding 27 (Reliability, done)
The flake was topic interference, not a daemon race: pools serve any create of their shape. Fixed in the topic; see [findings](../findings.md).

### 8. Directory transfer (Use, shipped)
`uploadDir`/`downloadDir` shipped in the JS SDK and CLI as per-file calls (no archive extraction from the untrusted guest). Remaining: Python parity and a live integration topic.

### 9. Published guest artifacts (Use and packaging, medium)
Build and publish the kernel and the test and production rootfs images as release assets from CI, with checksums, so a new host does not have to build them and a CI runner could start a daemon. Unblocks the third GitHub topology described in [ci-integration.md](ci-integration.md).
Verify: `scripts/setup.sh --download` on a clean host boots a sandbox.

### 10. Reflink and mounts that survive a reboot (Operations, low effort)
Persist the XFS loop mount with a systemd unit written by `create-reflink-volume.sh`, and make reflink the documented default for production setups.
Verify: reboot the dev box; `kiln doctor` and `/status` show reflink clones.

### 11. Exec error clarity (Use, small)
Found by running every example: `runCommand` on a binary that does not exist returns a raw 500 ("unexpected agent response: No such file or directory") instead of exit code 127 with a readable message, and `sandbox.mount()` on a guest without rclone gives a bare 400 "No such file or directory". Map a spawn failure in the guest agent to exit 127 with the error on stderr, and have the mount route say what is missing (FUSE kernel, rclone binary).
Shipped: the guest agent maps a spawn failure to exit 127 (not found, also a missing interpreter) or 126 (directory, no execute bit, bad format) with a shell-style line on stderr, for exec, timed exec, exec-stream and the PTY shell; other spawn errors stay `Response::Error`. `POST /sandboxes/:id/mounts` probes for `rclone` and `/dev/fuse` in one guest round trip before writing anything and answers `400` naming the missing piece. No protocol change. Verified live (second daemon, own bridge): topic `31-exec-spawn-failure` 34/34, and the SDK, CLI (exit 127/126) and MCP (`exit_code: 127`) show the message. Not done: a typed "command not found" in the SDKs (the exit code is the contract) and the PTY failure was only unit-tested, since a guest without any shell cannot be booted usefully.

### 12. Python parity and polish (Use, low; PTY, streamed exec and directory copy shipped)
Shipped in Python 0.6.0: `pty()`, `exec_stream()`/`attach_logs()`/`list_exec_streams()` and `upload_dir()`/`download_dir()`, sync and async, on one hand-rolled RFC 6455 client (`_ws.py`) that the tunnel shares. `get_status` already existed (0.4.0). Still open: a Python `doctor` equivalent (the CLI's `kiln doctor` has no SDK counterpart; decide whether one belongs in a client library at all).

## Later

A ranked security, isolation and operations backlog with sources is in [research-security-operations.md](research-security-operations.md).

Multi-node scheduling, snapshot encryption at rest, per-caller auth scoping, OCI image conversion, GPU sandboxes. Reasons and constraints are in [ideas-not-started.md](ideas-not-started.md).

Researched ranking of new feature candidates (process control, parallel fork, domain egress, credential injection, code interpreter first): [research-features.md](research-features.md).

## How work is run

Items are taken in order unless a bug blocks one. Each change follows the
checklist in [code-conventions.md](../code-conventions.md): code, tests, reference docs, changelog, roadmap row, and a finding or design note when there was a surprise. Numbers that move are recorded in [benchmarks.md](../benchmarks.md) from the benchmark suite, not from ad-hoc runs.

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

### 2. Warm path by default: a user-visible create is a resume (Latency, high)
A resumed sandbox answers its first command in about 8ms against 214ms cold, so the biggest user-visible win is not a faster boot but never booting on the request path. Add a `default_pool_warm_count` setting that keeps warm instances of the default profile ready without the operator creating a pool, replenished in the background, and report warm hits and misses in `/status` and `/metrics`.
Verify: benchmark suite first-command latency with the setting on and off; pool exhaustion falls back to cold create.

### 3. Snapshots must not hold a tap device each (Use and scale, shipped)
A held snapshot keeps its tap lease, and `DELETE` keeps a snapshot by default, so 32 snapshots exhaust a 32-tap pool and every create fails (found while stress-testing). The guest only needs a tap at resume. Firecracker can repoint a restored network interface (`network_overrides` on snapshot load), so release the tap at snapshot time and lease any free one at resume; keep the guest IP and MAC frozen. Surface held-snapshot tap usage in `/status` and warn before exhaustion.
Shipped: the lease is split into an `IpLease` (kept by a snapshot) and a `TapLease` (held only while a VM runs), resume repoints the interface with `network_overrides`, and `/status` reports `tap_pool.in_use`, `ip_pool` and warnings. Held snapshots are now bounded by the sandbox /24 (253 addresses) and disk, not by taps. Verified: 40 snapshots on a 32-tap pool, concurrent resumes, deny_all egress after a tap change, restart reconcile, topic `29-snapshots-hold-no-taps` (see finding 29). Not done: 100 snapshots (the /24 allows 253 but each full-RAM checkpoint costs its memory size on disk), and widening the /24 or giving a snapshot an IP only on demand.

### 4. Cold path: custom minimal init and trimmed kernel (Latency, high)
About 125ms of the 214ms is guest systemd userspace (not to be confused with Firecracker's 125ms init bound). Replace it with the guest agent as PID 1 (mounts, network from kernel args, then serve), drop unneeded kernel features, and signal readiness over vsock instead of polling. Target and levers are in the research note.
Verify: benchmark suite time-to-first-command; the full integration suite on both images.

### 5. Snapshot-take cost (Latency, medium)
Taking a snapshot writes the full memory file (310-727ms). Measure sandkiln's own overhead against Firecracker's, then evaluate diff snapshots and writing the memory file to a faster location. Matters for pools (replenish rate) and checkpoint-heavy agent loops.
Verify: benchmark suite snapshot latency; a resume from a diff-chain is bit-identical in a filesystem check.

### 6. Per-request exec timeout that kills the process (Use, medium)
`exec_timeout_secs` returns a 504 but the guest process keeps running. Add a per-request timeout field that also signals the process group in the guest.
Verify: integration topic where a timed-out `sleep` is gone from `ps`.

### 7. Pool claim accounting under load, finding 27 (Reliability, medium)
Reproduce with the parallel loop plus pool create, claim and stop; log claim and release per pool id; fix the race. See [findings](../findings.md).

### 8. Directory transfer (Use, shipped)
`uploadDir`/`downloadDir` shipped in the JS SDK and CLI as per-file calls (no archive extraction from the untrusted guest). Remaining: Python parity and a live integration topic.

### 9. Published guest artifacts (Use and packaging, medium)
Build and publish the kernel and the test and production rootfs images as release assets from CI, with checksums, so a new host does not have to build them and a CI runner could start a daemon. Unblocks the third GitHub topology described in [ci-integration.md](ci-integration.md).
Verify: `scripts/setup.sh --download` on a clean host boots a sandbox.

### 10. Reflink and mounts that survive a reboot (Operations, low effort)
Persist the XFS loop mount with a systemd unit written by `create-reflink-volume.sh`, and make reflink the documented default for production setups.
Verify: reboot the dev box; `kiln doctor` and `/status` show reflink clones.

### 11. Python parity and polish (Use, low)
Python has no PTY or streamed exec; add them, and add `status`/`doctor` equivalents where they make sense.

## Later

Multi-node scheduling, snapshot encryption at rest, per-caller auth scoping, OCI image conversion, GPU sandboxes. Reasons and constraints are in [ideas-not-started.md](ideas-not-started.md).

## How work is run

Items are taken in order unless a bug blocks one. Each change follows the
checklist in [code-conventions.md](../code-conventions.md): code, tests, reference docs, changelog, roadmap row, and a finding or design note when there was a surprise. Numbers that move are recorded in [benchmarks.md](../benchmarks.md) from the benchmark suite, not from ad-hoc runs.

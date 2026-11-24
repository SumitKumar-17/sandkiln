# Feature plan

The ordered plan behind the `ROADMAP.md` "what's next" list. Two goals pull in
parallel and every item is tagged with the one it serves:

- **Latency**: how fast a sandbox is ready (see [latency-floor.md](latency-floor.md) once written, and [benchmarks.md](../benchmarks.md)).
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

### 1. Warm path by default: a user-visible create is a resume (Latency, high)
A resumed sandbox answers its first command in about 8ms against 214ms cold, so the biggest user-visible win is not a faster boot but never booting on the request path. Add a `default_pool_warm_count` setting that keeps warm instances of the default profile ready without the operator creating a pool, replenished in the background, and report warm hits and misses in `/status` and `/metrics`.
Verify: benchmark suite first-command latency with the setting on and off; pool exhaustion falls back to cold create.

### 2. Snapshots must not hold a tap device each (Use and scale, high)
A held snapshot keeps its tap lease, and `DELETE` keeps a snapshot by default, so 32 snapshots exhaust a 32-tap pool and every create fails (found while stress-testing). The guest only needs a tap at resume. Firecracker can repoint a restored network interface (`network_overrides` on snapshot load), so release the tap at snapshot time and lease any free one at resume; keep the guest IP and MAC frozen. Surface held-snapshot tap usage in `/status` and warn before exhaustion.
Verify: create 100 snapshots on a 32-tap pool; resume succeeds; integration topic for it.

### 3. Cold path: custom minimal init and trimmed kernel (Latency, high)
About 125ms of the 214ms is guest userspace (systemd). Replace it with the guest agent as PID 1 (mounts, network from kernel args, then serve), drop unneeded kernel features, and signal readiness over vsock instead of polling. Target and levers are in the research note.
Verify: benchmark suite time-to-first-command; the full integration suite on both images.

### 4. Snapshot-take cost (Latency, medium)
Taking a snapshot writes the full memory file (310-727ms). Measure sandkiln's own overhead against Firecracker's, then evaluate diff snapshots and writing the memory file to a faster location. Matters for pools (replenish rate) and checkpoint-heavy agent loops.
Verify: benchmark suite snapshot latency; a resume from a diff-chain is bit-identical in a filesystem check.

### 5. Per-request exec timeout that kills the process (Use, medium)
`exec_timeout_secs` returns a 504 but the guest process keeps running. Add a per-request timeout field that also signals the process group in the guest.
Verify: integration topic where a timed-out `sleep` is gone from `ps`.

### 6. Pool claim accounting under load, finding 27 (Reliability, medium)
Reproduce with the parallel loop plus pool create, claim and stop; log claim and release per pool id; fix the race. See [findings](../findings.md).

### 7. Directory transfer and richer file API (Use, medium)
`uploadDir`/`downloadDir` (tar over the existing file calls) in both SDKs and the CLI. Needed by the CI action and agent workflows that move whole projects.
Verify: round-trip a tree with symlinks and modes; integration topic.

### 8. Published guest artifacts (Use and packaging, medium)
Build and publish the kernel and the test and production rootfs images as release assets from CI, with checksums, so a new host does not have to build them and a CI runner could start a daemon. Unblocks the third GitHub topology described in [ci-integration.md](ci-integration.md).
Verify: `scripts/setup.sh --download` on a clean host boots a sandbox.

### 9. Reflink and mounts that survive a reboot (Operations, low effort)
Persist the XFS loop mount with a systemd unit written by `create-reflink-volume.sh`, and make reflink the documented default for production setups.
Verify: reboot the dev box; `kiln doctor` and `/status` show reflink clones.

### 10. Python parity and polish (Use, low)
Python has no PTY or streamed exec; add them, and add `status`/`doctor` equivalents where they make sense.

## Later

Multi-node scheduling, snapshot encryption at rest, per-caller auth scoping, OCI image conversion, GPU sandboxes. Reasons and constraints are in [ideas-not-started.md](ideas-not-started.md).

## How work is run

Items are taken in order unless a bug blocks one. Each change follows the
checklist in [code-conventions.md](../code-conventions.md): code, tests, reference docs, changelog, roadmap row, and a finding or design note when there was a surprise. Numbers that move are recorded in [benchmarks.md](../benchmarks.md) from the benchmark suite, not from ad-hoc runs.

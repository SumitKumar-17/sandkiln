# Development

Everything that needs KVM, Firecracker or real networking runs on a KVM Linux
host; only pure-logic tests run anywhere. Contributor standard:
[AGENTS.md](../AGENTS.md). Conventions: [code-conventions.md](code-conventions.md).

## Build and test

```bash
scripts/dev.sh build                   # cargo build --release --workspace
scripts/dev.sh unit-test               # cargo test --workspace (no KVM needed)
scripts/dev.sh integration-test        # full API against a live sandkilnd, topics run concurrently
scripts/dev.sh bench-report 20         # fast regression check against /metrics, diffed with the last run
scripts/dev.sh load-test 10 20         # concurrency and latency under load
scripts/dev.sh docs-check              # links, docs.json, wording, size budgets
cargo bench -p sandkiln-vmm --bench vm_lifecycle   # boot, exec, snapshot, resume (real Firecracker)
```

- `setcap` does not survive a rebuild: re-run `scripts/host-setup/grant-net-admin.sh <binary>` after each build.
- `SANDKILN_INTEGRATION_TEST_ONLY=<substring>` runs one integration topic.
- The packages build with `npm run build` and test with `npm test` in `packages/sdk` and `packages/cli`.
- `scripts/dev.sh help` lists every subcommand.

## Repository layout

| Path | Contents |
| --- | --- |
| `core/crates/protocol/` | wire format shared by host and guest |
| `core/crates/guest-agent/` | static musl binary that runs inside the VM |
| `core/crates/vmm/` | drives Firecracker, rootfs clones, networking (host side) |
| `core/crates/store/` | durable sandbox-lifecycle history (sqlite) |
| `core/crates/daemon/` | axum HTTP API (`sandkilnd`) |
| `packages/sdk/`, `packages/python/`, `packages/cli/` | `sandkiln` (npm), `sandkiln` (PyPI), `kiln` CLI |
| `images/` | kernel and rootfs build scripts, agent injection |
| `scripts/` | setup, daemon control, tests, benchmarks |
| `website/`, `docs-site/`, `docs/` | project site (Astro), full docs (Mintlify), design notes and benchmarks |
| `examples/` | runnable reference projects |

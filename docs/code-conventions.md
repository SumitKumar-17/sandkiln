# Code conventions

The standard is [AGENTS.md](../AGENTS.md). This file is the practical recipe:
where code goes, what a file looks like, and the checklist for adding a
feature so every layer stays consistent as the codebase grows.

## Layout

| Layer | Rule |
| --- | --- |
| Crate | One crate per real boundary: `protocol` (wire), `guest-agent` (in-VM), `vmm` (Firecracker, no async), `store` (sqlite), `daemon` (HTTP). A crate does not depend "up". |
| `daemon/src/http/routes/` | One file per resource. A route file holds extraction, validation and response shaping only; reusable logic moves to `lifecycle/`, `snapshots/`, `storage.rs` or `sandkiln-vmm`. |
| `daemon/src/config/` | Settings are rows in the `settings.rs` registry; nothing reads `std::env` elsewhere. |
| Packages | The JS SDK is the reference client; the Python SDK and CLI mirror it. |

## A source file

1. `//!` module doc: what it owns and the invariant it protects, in a few lines.
2. Imports, then constants, then types, then `impl`, then free functions, then `#[cfg(test)] mod tests`.
3. Pure logic split from framework plumbing so it is unit-testable (`token_matches`, `evict_candidates`, `warnings`).
4. Split a file when it mixes independently workable concerns or passes about 300 lines for no structural reason.

Comments say *why*, only when non-obvious. Measurements, history and design
essays go to `docs/`; the comment links there.

## Errors, state and cleanup

- Errors cross a boundary as typed values (`AppError`, `io::Error` with context), never a bare string.
- Every resource has one owner and a scope guard for the gap between creation and hand-off (`OwnedFile`, lease guards).
- No lock is held across a guest call or any other blocking I/O: copy the handle out (`AppState::agent_endpoint`) and release.
- One `Mutex` guard per statement. A second lock of the same mutex in one expression deadlocks (found live in `/status`).
- Every intermediate failure of a multi-step operation leaves the system valid; test the failure path, not only success.

## Adding an HTTP endpoint

1. Route in `http/routes/<resource>.rs`, registered in `http/mod.rs` under the right auth layer.
2. Pure helpers with unit tests next to the code.
3. JS SDK method and type, then the Python method (sync and async), then the CLI command and formatter.
4. Integration topic in `scripts/integration-tests/NN-<name>.sh`, comparing exact ids, never global counts.
5. Reference docs (`docs-site/api-reference/`), `CHANGELOG.md`, `ROADMAP.md` status.
6. Verify against a real daemon and microVM; compiling is not proof.

## Adding a setting

Add a row to the `settings.rs` registry (key, default, group, doc, secret flag), consume it from `Config`, add a validation rule if it has a range, and update `sandkiln.example.toml` (a test fails if it drifts). `docs-site/self-hosting/configuration.mdx` links to the reference rather than copying it.

## Scripts

- `set -euo pipefail`, a usage block at the top, `--help`, and a nonzero exit with a useful message on failure.
- Shared helpers in `scripts/lib/`; load config through `lib/daemon-config.sh`, never hard-code paths or ports.
- Prefer readiness checks to sleeps. Use `pkill -x`, never `pkill -f`.
- A new script gets a `scripts/dev.sh` subcommand.

## Tests

| Kind | Where | Rule |
| --- | --- | --- |
| Unit | beside the code | pure logic, no KVM |
| Integration | `scripts/integration-tests/` | one topic per file, exact id sets |
| Regression for a fixed bug | the nearest topic or unit test | encodes the failure that was found |
| Benchmark | `cargo bench`, `scripts/bench-report.sh` | numbers recorded in `docs/benchmarks.md` |

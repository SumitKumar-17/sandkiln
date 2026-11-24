# Examples

Runnable reference projects on the `sandkiln` SDKs. Each directory is
standalone (its own manifest) and its README has the exact steps. Every one
needs a running `sandkilnd` ([`SELF_HOSTING.md`](../SELF_HOSTING.md)); the
SDK is `sandkiln` from npm or PyPI (0.13.0 / 0.4.0 at last verification).

Re-verify all of them against a daemon with
`scripts/dev.sh examples-smoke` (add `--published` to install the SDK from
the registry instead of this repository's build). It prints a pass, fail
or skip table and deletes everything the examples created.

"Verified" is the date the example was last run end to end on the dev box
(2026-11-24), against both this repository's build and the registry
packages. "Image" is what the guest image must contain beyond the agent.

| Example | Demonstrates | Needs | Run | Verified |
| --- | --- | --- | --- | --- |
| [`code-playground`](code-playground/) | run a file or stdin in a sandbox | Node 18+, image: python3 (`--lang js` needs node) | `node index.js hello.py` | yes |
| [`agent-runner`](agent-runner/) | Python SDK: run generated code, read a result file back | Python 3.9+, image: python3 | `python3 main.py` in a venv | yes |
| [`dev-server-preview`](dev-server-preview/) | reach a server inside a sandbox with `previewUrl()` | Node 18+, image: python3, daemon reachable from the browser | `node index.js` | yes (curl on the URL, SIGINT cleanup) |
| [`interactive-terminal`](interactive-terminal/) | live shell with `pty()` | Node 22+, a TTY | `node index.js` | partly: under a pseudo-terminal, not a real terminal emulator |
| [`snapshot-lifecycle`](snapshot-lifecycle/) | `snapshot()`, `fork()` independence, `resume()` consumes | Node 18+ | `node index.js` | yes |
| [`named-persistent-sandbox`](named-persistent-sandbox/) | `getOrCreate({ name })`, state kept across `stop()` | Node 18+ | `node index.js` | yes |
| [`pool-warm-start`](pool-warm-start/) | pre-warmed pool claims, fallback reporting | Node 18+ | `node index.js` (about 25 s) | yes |
| [`exec-stream-logs`](exec-stream-logs/) | detached command, live tail, reattach after exit | Node 22+ | `node index.js` | yes |
| [`exec-stream-logs-python`](exec-stream-logs-python/) | the same, in Python | Python 3.9+ | `python3 main.py` | yes |
| [`local-tunnel`](local-tunnel/) | sandbox reaches a service on this machine via `tunnel()` | Node 22+ | `node index.js` | yes |
| [`egress-policy`](egress-policy/) | `deny_all` and `allowCidrs` against a real target | Node 18+, image: ping, `EGRESS_EXAMPLE_TARGET_IP` | see its README | yes |
| [`remote-storage-mount`](remote-storage-mount/) | S3-compatible bucket mounted in the guest | FUSE kernel, rclone image, S3 endpoint | see its README | no: only validation and the failure path (see its README) |
| [`benchmark-suite`](benchmark-suite/) | client-side latency benchmarks, `compare` | Node 22.18+ | `npm start` or `scripts/dev.sh bench-suite` | yes (unit tests, short run, compare) |
| [`github-actions`](github-actions/) | run pull-request code in a deny-all microVM | `kiln` CLI, image: python3 | `run.sh` per its README | `run.sh` yes, on GitHub itself no |

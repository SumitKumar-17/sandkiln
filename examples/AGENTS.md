# AGENTS.md: examples

Read root `AGENTS.md` first. Real, runnable reference projects on the published SDKs (`sandkiln` on npm and PyPI), not
README snippets. Each subdirectory is standalone with its own manifest and is not a workspace member, so it looks like
what an external user would write.

## Contents

| Directory | Language | Shows |
|---|---|---|
| `code-playground/` | JS | run code from a file or stdin, print stdout, stderr, exit code |
| `agent-runner/` | Python | run a stand-in agent-generated script, read back a result file |
| `dev-server-preview/` | JS | start a server, print its browser URL via `previewUrl()` |
| `interactive-terminal/` | JS | live shell through `pty()` with raw-mode passthrough |
| `snapshot-lifecycle/` | JS | snapshot, two independent forks, then `resume()` consuming the snapshot |
| `named-persistent-sandbox/` | JS | `getOrCreate()` by name; state survives a default `stop()` |
| `pool-warm-start/` | JS | repeated pool claims, each reported as a clean claim or a health-check fallback |
| `exec-stream-logs/` | JS | detached command, live tail, then reattach after it finished |
| `remote-storage-mount/` | JS | mount an S3-compatible bucket, read and write through it, unmount |
| `egress-policy/` | JS | ping a caller-supplied IP under no policy, `deny_all`, and `deny_all` plus `allowCidrs` |
| `local-tunnel/` | JS | `tunnel()` a local HTTP server into the sandbox and `curl` it from inside |

## Conventions

- Every example needs a real running `sandkilnd`; nothing is mocked (see `SELF_HOSTING.md`).
- Stay minimal. An abstraction layer an example needs belongs in a library.
- Depend on the published `sandkiln` package, not in-repo paths. If an SDK method is not published yet, point at
  `packages/sdk`, say so in the README, and switch back to a version range after the release. A daemon feature with no
  SDK surface calls the HTTP API directly; do not invent SDK methods.
- Each `README.md` follows one template: what it does, requirements, run commands, configuration, notes.
- `egress-policy/` takes its target IP from the caller: a hardcoded public IP would pass falsely on a host with no
  outbound route.

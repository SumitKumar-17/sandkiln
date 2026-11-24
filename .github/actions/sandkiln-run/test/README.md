# Tests of the sandkiln-run action

Two layers, neither of which boots a microVM:

- `test-run.sh`: offline, run by `ci-actions.yml`. A stand-in `kiln` records
  its calls; it checks `run.sh`'s control flow (exit codes, cleanup,
  keep-on-failure, input validation).
- `stub-daemon.mjs` with `start-stub.sh` and `assert-requests.sh`: used by
  `.github/workflows/test-action.yml` to run the real action on a
  GitHub-hosted runner, with the real `sandkiln-cli` from npm talking HTTP and
  WebSocket to a local stub.

## The stub is a stub

`stub-daemon.mjs` is not `sandkilnd` and not a microVM. It implements only the
routes the action reaches through `kiln` (create, exec, exec-stream with the
log WebSocket, write-file, read-file, mkdir, chmod, delete), checks a bearer
token when `STUB_TOKEN` is set, and records every request as a JSON line in
`STUB_LOG` (never the token itself, only whether it matched).

Commands run as ordinary processes of the runner, as the runner user, in a
per-sandbox temp directory, with `/workspace` and `/tmp/sandkiln-*` mapped
into it and a minimal environment. So exit codes, `sleep`, `timeout` and
process-group kills behave truthfully, but there is no isolation, no egress
policy, no guest image, no real file API semantics and no guest agent. What
the tests prove is the action's wiring: inputs, the CLI install, outputs, exit
code propagation, cleanup and masking. They say nothing about isolation or
microVM behaviour.

`kiln` is installed by the action itself; the stub needs Node.js 18+ and `ws`
(`npm ci` in this directory, from the lockfile).

## Cancellation probe

`workflow_dispatch` with `cancel-probe: true` starts a job whose command
sleeps for 10 minutes. Cancel the run while it blocks (`gh run cancel <id>`) and
read the job's log: the "Show what the stub recorded" step lists whether the
final step sent the `DELETE`. The result is recorded in the action's README.

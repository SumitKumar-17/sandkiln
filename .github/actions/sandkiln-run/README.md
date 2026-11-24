# sandkiln-run

A composite action that runs a command in a fresh sandkiln microVM, with your
checked-out workspace uploaded into it, and always destroys the sandbox
afterwards. It needs a reachable `sandkilnd`; it does not start one (see
[CI integration](../../../docs/design-notes/ci-integration.md) for the
topologies and what GitHub-hosted runners can and cannot do).

```yaml
- uses: actions/checkout@v5
  with:
    persist-credentials: false
- uses: ./.github/actions/sandkiln-run
  with:
    daemon-url: ${{ secrets.SANDKILN_DAEMON_URL }}
    auth-token: ${{ secrets.SANDKILN_AUTH_TOKEN }}
    command: python3 -m unittest discover -v tests
```

## Inputs

| Input | Default | Meaning |
| --- | --- | --- |
| `daemon-url` | required | `https://` URL of `sandkilnd` (plain `http://` only for loopback). |
| `auth-token` | empty | Bearer token, masked in logs. Always set it for a remote daemon. |
| `command` | required | Shell script run with `sh` from `/workspace` inside the sandbox. |
| `image` | daemon default | Registered image id to boot. The image needs `sh` and `tar`. |
| `working-directory` | `.` | Directory (relative to the workspace) whose contents are uploaded. |
| `exclude` | empty | Extra comma-separated tar excludes. `.git` is always excluded. |
| `egress` | `deny_all` | `default`, `deny_all` or `allow_all`. |
| `allow-cidrs` | empty | Comma-separated CIDRs allowed under `deny_all`. |
| `timeout` | `600` | Seconds before the job is killed in the sandbox and the step fails with exit code 124. Needs `timeout` (coreutils or busybox) in the image. |
| `keep-on-failure` | `false` | Leave the sandbox running when the command fails. You must remove it. |
| `cli-version` | `0.14.0` | `sandkiln-cli` version installed from npm. |
| `install-cli` | `true` | `false` when `kiln` and Node.js 22+ are already on `PATH`. |

## Outputs

`sandbox-id`, and `exit-code` (the command's own; 124 on timeout, 125 when the
sandbox never reported one).

## How it behaves

- The step fails when the command exits non-zero, with that code in
  `exit-code`. A dropped output stream is a failure (125), never a pass.
- A final `if: always()` step destroys the sandbox, also after a failure,
  timeout or cancellation (observed: see "Cancellation" below). Sandboxes are tagged `github-run` and
  `github-repo`, so `kiln sandbox ls` shows orphans if a runner dies.
- Output arrives live (`exec-stream`). The job runs under `timeout` inside the
  sandbox, which kills its whole process group at the limit; the streaming API
  has no limit of its own. An image without `timeout` only gets a client-side
  limit that abandons the stream, and the job stops when the sandbox is
  destroyed.
- The workspace goes up as one `tar.gz` over the file API, which base64
  encodes it: exclude build output and caches with `exclude`.
- Nothing from the runner's environment enters the sandbox except the files
  and the command. Never put secrets in `command` for untrusted code.

## Cancellation

Observed on a GitHub-hosted runner (2026-11-24, runner 2.338.2, `ubuntu-latest`): cancelling
the run (`gh run cancel`) while the command was blocked on `sleep 600`
marked the action's main step `cancelled` about 12 seconds after the request,
and the final step (`if: always()`) then **did run** and destroyed the
sandbox (`DELETE /sandboxes/<id>?keep=false`, 204). Later steps with
`if: always()` ran as well; the job and run ended `cancelled`. Not observed:
a second (force) cancel, a job `timeout-minutes` expiry, and a runner that
dies, none of which are known to run the final step. Keep the `github-run`
tag in mind for those: `kiln sandbox ls` shows the orphans.

## Files and tests

- `action.yml`: inputs, outputs, the step order.
- `run.sh`: all the logic (`run` and `cleanup`), runnable outside GitHub.
- `test/test-run.sh`: offline control-flow tests against a stand-in `kiln`.
- `test/`: a stub daemon and the helpers behind
  `.github/workflows/test-action.yml`, which runs this action on a
  GitHub-hosted runner; see [test/README.md](test/README.md).
  Live verification against a real daemon is in the [example](../../../examples/github-actions/README.md).

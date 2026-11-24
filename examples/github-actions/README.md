# GitHub Actions: test a pull request inside a microVM

A reference for running CI work in a sandkiln sandbox instead of on the
runner: the [`sandkiln-run`](../../.github/actions/sandkiln-run/README.md)
composite action uploads the checked-out code, runs a command with all
outbound network denied, streams the output, and destroys the sandbox.

- `sample-project/`: a small Python project with unit tests plus an
  isolation test (`tests/test_isolation.py`) that fails if the sandbox can
  reach the network.
- `workflows/run-pr-code-in-a-microvm.yml`: pull requests, runner talks to a
  remote `sandkilnd` using the secrets `SANDKILN_DAEMON_URL` and
  `SANDKILN_AUTH_TOKEN`.
- `workflows/run-on-self-hosted-runner.yml`: the same job on a self-hosted
  runner that is also the KVM host (daemon on `127.0.0.1:7777`).

Copy a workflow into your repository's `.github/workflows/`, and copy
`.github/actions/sandkiln-run/` with it (or reference it from wherever you
publish it).

## Requirements

A running `sandkilnd` reachable from the job, with `auth_token` set if it is
not loopback-only ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). Never expose
an unauthenticated daemon. GitHub-hosted runners are only clients here: see
[`docs/design-notes/ci-integration.md`](../../docs/design-notes/ci-integration.md)
for why the daemon cannot be assumed to run on them.

## Try the action's script without GitHub

`run.sh` takes GitHub's `INPUT_*` variables, so it runs anywhere `kiln` does
(this is how it was verified). Names with a hyphen cannot be set with
`export` in bash, so pass them through `env`:

```
cd examples/github-actions
export RUNNER_TEMP=$(mktemp -d) GITHUB_OUTPUT=$(mktemp)
run() { env 'INPUT_DAEMON-URL=http://127.0.0.1:7777' INPUT_EGRESS=deny_all \
  INPUT_WORKING-DIRECTORY=sample-project GITHUB_WORKSPACE="$PWD" \
  INPUT_COMMAND='python3 -m unittest discover -v tests' \
  ../../.github/actions/sandkiln-run/run.sh "$@"; }
run run; echo "exit $?"; cat "$GITHUB_OUTPUT"
run cleanup
```

## Config

| Secret | Meaning |
| --- | --- |
| `SANDKILN_DAEMON_URL` | `https://` URL of the daemon |
| `SANDKILN_AUTH_TOKEN` | its `auth_token` |

## Verified, and not

Verified against a real daemon: success, failure and timeout exit codes,
cleanup leaving the sandbox list unchanged, deny-all egress, and `actionlint`
on the workflows. Not verified: a run on GitHub itself, and authenticated
requests. See the design note's verification record.

## Security notes

- Fork pull requests get no secrets, so the main workflow skips them.
- Do not run a self-hosted runner on untrusted pull requests of a public
  repository.
- `.git` is never uploaded; use `persist-credentials: false` on checkout.

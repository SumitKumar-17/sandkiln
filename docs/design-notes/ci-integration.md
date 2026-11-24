# CI integration

Context: can sandkiln be used from GitHub Actions, and in which shape? This
note records what was researched (2026-11-24), the three supported
topologies, and what is verified. The how-to is the
[CI guide](../../docs-site/guides/run-ci-jobs-in-a-sandbox.mdx); the runnable
parts are [the action](../../.github/actions/sandkiln-run/README.md) and
[the example](../../examples/github-actions/README.md).

## Research: KVM on GitHub-hosted runners

Sources were fetched on 2026-11-24. Most evidence about `/dev/kvm` is
community reports, not GitHub documentation; treat it as such.

| Question | Finding | Source |
| --- | --- | --- |
| Does GitHub support nested virtualization? | Docs: "While nested virtualization is technically possible while using runners, it is not officially supported." Any use "is experimental and done at your own risk", with no stability, performance or compatibility guarantee. | [About GitHub-hosted runners](https://docs.github.com/en/actions/concepts/runners/github-hosted-runners) |
| Does that page mention KVM, public vs private repos, or CPU architecture? | No to all three. It names only operating systems and says larger runners need GitHub Team or Enterprise Cloud. | same page |
| Larger runners | A community thread cites a GitHub changelog dated 2023-02-23 adding hardware-accelerated nested virtualization to larger Linux runners. A user reported KVM working on a large runner in 2023-05. | [Discussion 8305](https://github.com/orgs/community/discussions/8305) |
| Standard runners | GitHub changelog 2024-04-02 says hardware acceleration reached 2-vCPU Linux runners (before that, 4 vCPU and up) and gives the udev rule below. A 2025-10-27 comment in the same thread reports KVM working after only the udev rule. | [Changelog 2024-04-02](https://github.blog/changelog/2024-04-02-github-actions-hardware-accelerated-android-virtualization-now-available/), Discussion 8305 |
| Public vs private repositories | No authoritative source found. Whether a runner's size or KVM availability differs by repository visibility is unverified. | none |
| Architecture | x86_64 only is reported; extensions are needed on the host. Arm runners were not found to expose KVM. | Discussion 8305 (2023-02-17 comment) |
| Does Firecracker run there? | Firecracker needs read/write access to `/dev/kvm`, and its getting-started guide uses only bare-metal cloud instances, because the provider supports KVM only on those. It does not mention nested virtualization. One third-party analysis says neither Firecracker nor Cloud Hypervisor is validated on GitHub's nested hosts. | [Firecracker getting started](https://github.com/firecracker-microvm/firecracker/blob/main/docs/getting-started.md); [software-factory issue 10](https://github.com/sapientpants/software-factory/issues/10) (unverified, one person) |
| Third-party runners | Some outside providers sell Linux runners with nested virtualization as a runner label (a vendor claim, not a GitHub feature). Not evaluated here. | search results, 2026-11-24 |

The udev rule used by the community and in the 2024 changelog, written to
`/etc/udev/rules.d/99-kvm4all.rules` before reloading and triggering udev:

```
KERNEL=="kvm", GROUP="kvm", MODE="0666", OPTIONS+="static_node=kvm"
```

Conclusion: a hosted runner may expose `/dev/kvm` and may let a nested guest
boot, but GitHub does not support it, availability by runner type and
repository visibility is not documented, and nobody has shown Firecracker
working on it. We did not try it (no GitHub account was used for this work).

## What sandkilnd needs from a host

`sandkilnd` is not just a Firecracker launcher. It also needs a pre-created
tap pool (`ip tuntap add` needs real root), `CAP_NET_ADMIN` on the daemon
binary, a bridge with NAT, a DNS forwarder, a Firecracker binary, a guest
kernel and a rootfs with the guest agent injected
([SELF_HOSTING.md](../../SELF_HOSTING.md)). The kernel and rootfs are built
locally by `images/` scripts; **no kernel or rootfs artifact is published**.
Nothing in this repository downloads a prebuilt one.

## Topologies

### A. CI job talks to a remote `sandkilnd` over HTTPS

The runner is just a client. It installs `kiln`, uploads the workspace and
reads the result back through the HTTP API.

- Works on any runner (hosted, larger, self-hosted, any architecture), and on
  every plan, because the runner needs neither KVM nor privileges.
- Needs a daemon reachable from GitHub's runners and `auth_token` set. Hosted
  runner IP ranges are public and change, so an IP allow-list is not a
  practical control for them; use a TLS-terminating reverse proxy in front of
  the daemon, or a tunnel, and rely on the bearer token plus rate limiting.
- Security guidance: never expose an unauthenticated daemon (anyone who can
  reach it can run code on your host); put the token in a repository or
  environment secret, never in a workflow file; use a dedicated token for CI;
  restrict by IP or a private tunnel wherever the runner's address is stable
  (self-hosted runners); use environments with required reviewers for
  anything that is not trusted.
- Cost: network latency on every call, daemon capacity shared with CI load,
  and one more public endpoint to keep patched. Concurrency is bounded by the
  daemon's ceilings, not by the runner count.
- Fork pull requests get no secrets, so they cannot reach the daemon.
  `pull_request_target` would give them access; see the guide for why that is
  only acceptable when nothing from the pull request runs on the runner.

This is the topology the example's main workflow uses.

### B. Self-hosted runner on a KVM host running `sandkilnd` locally

The runner and the daemon are the same machine; the action talks to
`http://127.0.0.1:7777`.

- No public endpoint and no secret needed (a token is still advisable if
  other users share the host). Lowest latency; the daemon's benchmarks apply
  directly.
- You operate the host: patching, disk, tap pool, the runner service. The
  runner user needs no KVM access of its own, only network access to the
  daemon; the daemon runs unprivileged as in [SELF_HOSTING.md](../../SELF_HOSTING.md).
- A persistent self-hosted runner executing untrusted pull-request workflows
  is a standing foothold on that host. Do not trigger it from fork pull
  requests on a public repository. Sandkiln isolates the code it runs; it
  does not isolate the runner process itself.
- Needs the runner to have `tar`, `bash`, `timeout`, and Node.js 22 or newer
  (the action installs Node itself through `actions/setup-node`).

### C. GitHub-hosted runner starts `sandkilnd` itself

Possible only if the runner exposes usable `/dev/kvm` (see the research
above), and not currently supported by this project.

What would have to exist, none of which does today:

1. A published, versioned guest kernel and a rootfs with the guest agent, or
   a build step that produces both inside the job. The `images/` scripts
   build them from source and are far too slow to repeat per job without a
   cache, so a cached or published artifact would be required.
2. A Firecracker release download pinned by checksum.
3. A non-root way to get the tap pool: `ip tuntap add` needs root, which a
   hosted runner does allow through `sudo`, so a setup script
   (`scripts/host-setup/`) would have to be adapted to run per job.
4. A readiness check that fails the job cleanly when `/dev/kvm` is absent,
   because availability is not guaranteed.
5. Measurement. Nested virtualization costs performance, and boot latency
   numbers from bare metal do not transfer.

Until then C is an open question, recorded in
[ideas-not-started.md](ideas-not-started.md), not a feature. No workflow in
this repository pretends to do it.

## Decisions

- Ship a composite action built on the `kiln` CLI rather than a JavaScript
  action: the CLI already owns auth, errors and exit codes, and keeping the
  logic in one shell script means it is runnable and testable outside GitHub.
- Upload the workspace as one `tar.gz` through the existing file API and
  extract it with `exec`, instead of adding a daemon endpoint. The JSON file
  API base64-encodes the body, so very large workspaces are a poor fit; the
  action does not cap size, so exclude build output with `exclude`.
- Stream with `exec-stream`, and also write the command's exit code to a file
  in the guest and read it back. The streaming client reports `0` when its
  connection closes without ever seeing an exit notice, so trusting it alone
  could turn a dropped connection into a green build. A missing file maps to
  exit code 125.
- Timeout is enforced by the client (`timeout`, exit code 124). The daemon
  has no exec timeout or kill, so the command may keep running until the
  sandbox is destroyed, which the cleanup step does.
- Always exclude `.git`: `actions/checkout` can leave a token in
  `.git/config`, and the sandbox is where untrusted code runs.
- Default egress is `deny_all` in the action. The sandbox gets no secrets and
  no network unless the workflow asks for them.
- Cleanup is a final `if: always()` step. Composite actions have no post
  step, so the sandbox lives until the action's last step, not the job's end.
  A cancelled job should still run it (unverified here); a runner machine that dies does not, so tag
  every sandbox (`github-run`, `github-repo`) to make orphans findable.

## Verification record

Verified on the dev box against the real daemon and microVMs, 2026-11-24:

- `run.sh run` and `cleanup`, driven with `INPUT_*`, `GITHUB_OUTPUT`,
  `GITHUB_WORKSPACE`, `RUNNER_TEMP`: success (exit 0, five tests passed),
  failure (exit 3 propagated), timeout (exit 124), keep-on-failure (sandbox
  left, then removed by hand), insecure URL rejected (exit 2). After the
  success, failure and timeout runs the daemon's sandbox list was identical
  to before.
- The deny-all policy under test: the sample's outbound connection test
  passed inside the sandbox.
- The published `sandkiln-cli@0.12.0` from npm installs and creates, execs
  and destroys a sandbox (the box ran the in-repo build for the runs above).
- A daemon restart by another user mid-run made `cleanup` report a failed
  destroy and name the manual command, which is the intended failure path.
- `actionlint` 1.7.7 passes on every workflow and the action, and rejects a
  deliberately wrong expression. `.github/actions/sandkiln-run/test/test-run.sh`
  passes offline against a stand-in `kiln` (seven scenarios).

Not verified, because it needs GitHub:

- The workflows and the action running on a real GitHub runner, including
  `actions/setup-node`, `$GITHUB_PATH`, masking, `if: always()` on a
  cancelled job, and the `secrets` context.
- Authenticated use (`auth-token`): the box daemon runs without an
  `auth_token`, so the bearer path and masking were not exercised end to end.
- Any hosted-runner KVM behavior, topology C, and fork pull requests.

## Open questions

- Does a hosted runner boot a Firecracker guest at all, and at what latency?
- A published guest kernel/rootfs release would unlock C; not planned.
- A daemon-side exec timeout and kill would replace the client-side one
  (also listed under the MCP server's gaps).

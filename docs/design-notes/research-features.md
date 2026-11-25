# Research: which features to build next

Written 2026-11-25. Question: what do people building agents, coding assistants,
code-execution backends, CI runners, online IDEs and evaluation or RL harnesses
need from a sandbox runtime that sandkiln does not yet give them, and in what
order should the gaps be closed? Method: read the repo (the sources named per
feature below), then search open-source sandbox projects, their issue trackers,
the MCP and Jupyter specs, papers and engineering write-ups. Other systems are
described generically; only open-source projects, standards and papers are
named. Anything not verified is marked **unverified** and collected at the end.
Roadmap context: [feature-plan.md](feature-plan.md), [ideas-not-started.md](ideas-not-started.md).

## Method and scoring

Score = (user value x differentiation) / effort. Value and differentiation are
1-5 judgements from the evidence below (differentiation: how much it leans on
what a self-hosted microVM runtime with snapshots can do that a container
runtime cannot). Effort weights: S = 1, S/M = 1.5, M = 2, M/L = 3, L = 4, XL = 5.
These are estimates, not measurements; the ordering matters more than the numbers.

## Ranked table

| # | Feature | Value | Diff. | Effort | Score | Risk | Verdict |
| --- | --- | --- | --- | --- | --- | --- | --- |
| F1 | Parallel fork and trajectory API (N-way branch, rollback in SDK/CLI/MCP) | 5 | 5 | M | 12.5 | High (guest network identity) | Top 5 (#2) |
| F2 | Credential injection outside the guest | 5 | 4 | M | 10.0 | Med (TLS interception) | Top 5 (#4) |
| F4 | Process control: exec kill, stdin, PTY resize, stream timeout | 4 | 2 | S | 8.0 | Low | Top 5 (#1) |
| F3 | Domain-level egress allow list (host-side proxy) | 5 | 3 | M | 7.5 | Med | Top 5 (#3) |
| F5 | Stateful code interpreter (Jupyter protocol) plus WebSocket preview | 5 | 3 | M | 7.5 | Med | Top 5 (#5) |
| F6 | Package and git caching (pull-through mirror, cache drives) | 4 | 3 | M | 6.0 | Med (cache poisoning) | Next |
| F7 | SDK ergonomics: idempotency keys, retries, typed iterators, `runTests` helpers | 4 | 2 | S/M | 5.3 | Low | Next |
| F8 | Inbound access: per-sandbox preview tokens, stable URLs | 4 | 2 | S/M | 5.3 | Med | Next |
| F9 | Templates: declarative build from a recipe with layer caching | 5 | 4 | L | 5.0 | Med | Design now, build after top 5 |
| F10 | Volumes: clone, snapshot, quota for drives | 4 | 2 | M | 4.0 | Low | Later |
| F11 | Audit trail and OpenTelemetry traces | 3 | 2 | S/M | 4.0 | Low | Later |
| F12 | Events: file watch, lifecycle webhooks | 3 | 2 | M | 3.0 | Low | Later |
| F13 | Integrations: agent-framework adapters, IDE extension | 3 | 1 | S | 3.0 | Low | Opportunistic |
| F14 | Scoped API keys, quotas, usage metering | 4 | 2 | M/L | 2.7 | Med | Only when a daemon is shared |
| F16 | Multi-node and regions | 4 | 3 | XL | 2.4 | High | Last (already stated) |
| F15 | Headless browser and desktop images | 3 | 2 | M/L | 2.0 | Low | After F5 and F8, as an image |
| F17 | GPU, git-native filesystem, workflow engine | - | - | - | - | - | Do not build (see end) |

## Top 5, in the order to build them

1. **F4 Process control.** Smallest change, already item 4 of the roadmap, and the
   gap is a correctness hole (a streamed process cannot be stopped or bounded),
   not a nice-to-have. It also makes every later long-running feature safe.
2. **F1 Parallel fork and trajectories.** Highest score and the one place where
   snapshot-and-fork is a structural advantage. Ship the cheap slice first
   (SDK, CLI and MCP surface for lineage and restore, which the daemon already
   has), then the hard part (live parallel forks).
3. **F3 Domain egress proxy.** Needed on its own (package installs by name, not
   by CIDR) and is the prerequisite component for F2.
4. **F2 Credential injection.** Same proxy, plus a per-host rule table. Strongest
   security story of the set, and easy to get subtly wrong, so it follows a
   proxy that has already been run for a while.
5. **F5 Code interpreter and WebSocket preview.** The most requested agent
   ergonomics item; needs the preview path to carry WebSockets, which F8 and the
   browser image also need.

Why not F9 (templates) in the top 5 when it has the highest raw value: it is L,
it touches the image, pool and snapshot layers at once, and F6 captures most of
its measured benefit (dependency cold-start) at a third of the cost. Write its
design note next; build F6 first and let its numbers decide how much of F9 is needed.

## Feature by feature

Each entry: **Want** (evidence), **Today** (repo), **Gap**, **Design**, **Effort and risk**, **Verify**.

### F1 Parallel fork and trajectory API

- **Want.** Tree search and RL rollouts need cheap checkpoint, rollback and N-way fork of live state. A systems paper reports that copying whole sandbox state costs "hundreds of milliseconds to seconds" per checkpoint and proposes diff-based checkpoints with fork from a frozen template ([DeltaBox, arXiv 2605.22781](https://arxiv.org/abs/2605.22781)); another proposes snapshot, rollback and fork as the three primitives of agent state ([Planarian, arXiv 2609.35366](https://arxiv.org/abs/2609.35366)); an open-source Firecracker RL environment advertises copy-on-write forks, incremental snapshots and 122 ms amortised fork ([AgentENV](https://kvcache.ai/blog/agentenv-open-sourced/), 2026-07-27); another open-source project forks 100 microVMs from a warm parent ([forkd](https://github.com/cristiannegru/forkd), its own numbers). A restored agent can repeat external side effects ([Safe to Resume?, arXiv 2608.29381](https://arxiv.org/abs/2608.29381)).
- **Today.** `POST /snapshots/:id/fork` allows one live fork per snapshot (409 otherwise); lineage and `snapshots/history/:id/restore` exist in the HTTP API only; MCP has `sandbox_fork` ([snapshots.md](snapshots.md), [MCP reference](../../docs-site/api-reference/mcp/overview.mdx), [ROADMAP.md](../../ROADMAP.md) "Time-travel restore, lineage").
- **Gap.** Parallel branches (the blocker is that two live forks would present one frozen guest IP and MAC, see "Why fork is single-live" in [snapshots.md](snapshots.md)); no SDK or CLI for lineage and time-travel; no tree view.
- **Design.** Slice 1 (S): `listSnapshots` filters, `history`, `restore` and a lineage tree in both SDKs, `kiln snapshot tree|restore`, MCP `sandbox_restore`. Slice 2 (M): clone each fork into its own network namespace with the snapshot's tap name and guest IP, joined to the host by a veth pair with NAT, which is the approach the VMM project documents for clones ([network-for-clones.md](https://raw.githubusercontent.com/firecracker-microvm/firecracker/main/docs/snapshotting/network-for-clones.md); it states it is "not performant or secure" as shipped, so the bridge port isolation and the per-sandbox egress chain must be re-applied inside the namespace). `ResumeFixup` already reseeds entropy and clock per clone ([feature-plan.md](feature-plan.md) item 1). Add `POST /snapshots/:id/fork` body `count`, returning ids. Document that external side effects are not rolled back; `sandbox.exec` results carry no idempotency guarantee.
- **Effort and risk.** M, high: network identity is the open problem the notes already call "genuinely open". Keep the single-live rule behind a flag until the namespace path passes the suite.
- **Verify.** Fork 8 children of one snapshot concurrently, each runs a command and reaches the gateway; two children cannot reach each other (port isolation); `deny_all` holds in every child; exact id sets per topic (as `18-pool.sh`); delete parent while children live returns 409; benchmark suite fork-to-first-command p50/p99.

### F4 Process control

- **Want.** Long-running sessions need kill, stdin, resize and bounded runtime; a protocol for long operations that survive reconnects is now an MCP extension with states `working`, `completed`, `failed`, `cancelled` ([MCP tasks](https://modelcontextprotocol.io/extensions/tasks)). Remote-runtime projects expose persistent sessions as a first-class concept ([SWE-ReX](https://github.com/SWE-agent/SWE-ReX)).
- **Today.** `exec` has `timeout_secs`; `exec-stream` has no limit, no kill endpoint ("the guest agent has no registry to signal a child by id"), PTY size is set once ([preview.md](preview.md), [ROADMAP.md](../../ROADMAP.md) next item 4, [HTTP sandboxes](../../docs-site/api-reference/http-api/sandboxes.mdx)).
- **Gap.** Cannot stop a streamed process, cannot send stdin to one, cannot resize a PTY, sessions are lost on resume and daemon restart.
- **Design.** Protocol: a process registry in the guest agent keyed by session id (spawn in its own process group as `exec` already does); requests `Signal{id, sig}`, `Stdin{id, bytes}`, `Resize{cols, rows}`. Daemon: `DELETE /sandboxes/:id/exec-stream/:sid` (SIGTERM, 2 s grace, SIGKILL), `timeout_secs` on `exec-stream`, a resize control frame on the PTY socket. SDKs and CLI: `session.kill()`, `kiln sandbox kill`; MCP `process_kill`. Keep the registry in guest memory so it survives a snapshot (it is process state).
- **Effort and risk.** S, low. Old guest agents ignore new fields; follow the `timeout_secs` precedent.
- **Verify.** Kill a `sleep` tree and assert no child is left (as topic 30); stream timeout answers in the limit; resize visible in `stty size`; a session survives snapshot and resume.

### F3 Domain-level egress allow list

- **Want.** Agents need `pip` and `npm` to work and little else; CIDR lists do not express that because registries sit behind shared CDNs. Projects hit it as a default-deny network breaking installs and fix it with a default registry allow list ([AGENT8088 PR 68](https://github.com/palindrome-rl/AGENT8088/pull/68)). Open-source microVM sandboxes implement SNI inspection and relay or drop ([microsandbox-network](https://docs.rs/crate/microsandbox-network/0.5.7/source/lib/proxy.rs)). Enforcement must be at the network layer: an allow list that depends on `HTTPS_PROXY` is advisory when the agent can unset it ([issue 92](https://github.com/hermeum/hermes-agent-operator/issues/92)). A write-up argues allow lists should be default-deny at the network level ([DEV post](https://dev.to/the_seventeen/the-outbound-sandbox-why-ai-agents-need-network-level-allowlists-1d0k)). The open-source Firecracker platform lists "SNI/Host-inspecting domain allow and deny lists" with a per-sandbox nftables firewall ([infra README](https://raw.githubusercontent.com/e2b-dev/infra/main/README.md)).
- **Today.** IP/CIDR allow and deny per sandbox in one iptables chain, IPv4 only, no ports or domains ([egress-firewall.md](egress-firewall.md), [ROADMAP.md](../../ROADMAP.md) "Egress policy").
- **Gap.** `allow_domains`, port rules, IPv6.
- **Design.** A host-side transparent proxy (new crate, `core/crates/egress-proxy`, one task per daemon, not per sandbox). The per-sandbox chain DNATs TCP 80 and 443 from that sandbox's uplink to the proxy and drops everything else under `deny_all`; the proxy reads the TLS ClientHello SNI (or HTTP Host), checks it against that sandbox's rule set (looked up by source IP, which is already a lease), then relays or resets. Require SNI and Host to match for HTTP, refuse missing SNI, resolve the name itself and connect to that address (not the client's destination) to stop DNS and IP games. Rules: `egress.allow_domains` with `*.suffix` support, plus `allow_ports`. Fail closed if the proxy is down. Known limit to document: domain fronting through an allowed CDN and exfiltration to an allowed host stay possible (the issue and DEV post above both say so). SDK `egress.allowDomains`, Python `egress_allow_domains`, CLI `--allow-domain`, MCP `allow_domains`.
- **Effort and risk.** M, medium: non-HTTP protocols are blocked rather than inspected; QUIC (UDP 443) must be dropped so clients fall back.
- **Verify.** Integration topic: allowed host reachable, another host on the same CDN IP refused, a connection with a spoofed Host header refused, UDP 443 dropped, policy preserved across snapshot, resume and fork, proxy killed mid-test fails closed; measure added latency with the benchmark suite.

### F2 Credential injection

- **Want.** The same pattern recurs: real keys live in a proxy outside the sandbox; the guest holds a placeholder and the proxy adds the credential only for allowed hosts ([blog analysis of the secret-proxying gap](https://williamzujkowski.github.io/posts/2026-07-02-agentic-ai-sandbox-secret-proxying-gap/), [issue 92](https://github.com/hermeum/hermes-agent-operator/issues/92)). The open-source platform's design note: "no value ever crosses the API, a log, or a span" ([infra README](https://raw.githubusercontent.com/e2b-dev/infra/main/README.md)).
- **Today.** Env vars and files are set by the caller, so a secret in the guest is readable by any code in it; the MMDS service carries only `{id, name, tags}` ([tags-and-metadata.md](tags-and-metadata.md)). No secret store.
- **Gap.** A way to use a credential without the guest ever holding it.
- **Design.** Daemon-owned secret store (sqlite table in `sandkiln-store`, encrypted with a key from the config file; names only returned by the API). `POST /secrets`, and at create `secrets: [{name, hosts, header}]`. The proxy from F3 terminates TLS only for hosts that have a rule, using a per-daemon CA whose public certificate is added to the guest trust store at image build (private key never leaves the host), and tunnels all other hosts by SNI untouched. It rewrites the configured header (for example `Authorization`) from the placeholder to the secret. Guest sees an env var holding a random placeholder. Responses are not scrubbed (a service that echoes the key back defeats it; document it). Clients that pin certificates fail closed.
- **Effort and risk.** M on top of F3; medium: the CA, the store encryption key and log redaction are the security-critical parts and need a review pass ([security-hardening.md](security-hardening.md)).
- **Verify.** Guest `env`, `/proc`, a full file search and a packet capture inside the guest never show the secret; the upstream sees it; a request to a host without a rule carries nothing; secret absent from daemon logs and `/metrics`; survives snapshot and fork with the same placeholder; deleting the secret stops injection for live sandboxes.

### F5 Stateful code interpreter and WebSocket preview

- **Want.** Agent frameworks ship a stateful kernel executor because variables must persist across calls, unlike a per-block process ([AutoGen Jupyter executor](https://microsoft.github.io/autogen/docs/topics/code-execution/jupyter-code-executor)); the Jupyter gateway exposes kernels over REST and WebSocket ([Jupyter Kernel Gateway](https://jupyter-kernel-gateway.readthedocs.io/)); containerised sandboxes start a gateway inside the sandbox for multi-turn tool calls ([ms-enclave](https://ms-enclave.readthedocs.io/en/latest/guides/notebook-sandbox/)). Rich-output (images, HTML) handling over the gateway: **unverified**.
- **Today.** A model can run `python` per call via `exec`; a dev-server preview proxies HTTP only, no WebSocket ([preview.md](preview.md), [ROADMAP.md](../../ROADMAP.md) "Preview, tunnel, PTY").
- **Gap.** No persistent interpreter, no structured outputs, and a kernel gateway cannot be reached through the preview path.
- **Design.** Ship it as a guest image feature plus a thin daemon route, not as new guest-agent logic: the universal image gets `ipykernel`; `POST /sandboxes/:id/code {language, code, context_id}` starts or reuses a kernel (guest agent spawns it through the F4 registry), the daemon speaks the Jupyter messaging protocol to it over the existing vsock tunnel, and returns `{stdout, stderr, results[{mime, data}], error}` and a streamed variant. Kernel state is process memory, so it survives snapshot, resume and fork for free (the differentiator over container runtimes). Separately, add WebSocket upgrade proxying to `/preview/:port` (reuse the tunnel's hand-rolled RFC 6455 code). SDK `runCode`, Python `run_code`, CLI `kiln sandbox code`, MCP `sandbox_run_code`.
- **Effort and risk.** M, medium: image size, kernel timeouts and interrupt (needs F4 signal).
- **Verify.** Define a variable in call 1, read it in call 2; snapshot, resume, read again; `matplotlib` figure returns `image/png`; interrupt a loop; a WebSocket echo server through preview.

### F6 Package and git caching

- **Want.** Setup dominates short agent sessions: sandboxed sessions keep cold dependency caches ([agentty PR 1375](https://github.com/agentty-xyz/agentty/pull/1375)); a shared snapshot that new agents clone cut `npm ci` from 37-46 s to 13-14 s on one machine ([agentbox PR 470](https://github.com/leciric/agentbox/pull/470), anecdotal); cross-run caching trades speed for cache poisoning risk ([fullsend issue 534](https://github.com/fullsend-ai/fullsend/issues/534)); clone and install time as the dominant cost ([write-up](https://rywalker.com/agent-speed-is-infrastructure)).
- **Today.** Read-only shared drives, snapshots, pools ([drives-and-storage.md](drives-and-storage.md)); nothing aimed at dependency reuse.
- **Gap.** Each fresh sandbox downloads everything.
- **Design.** Two parts. (a) A pull-through mirror (npm, PyPI, and a git object cache) listening on the bridge gateway, configured in the image via `npmrc`, `pip.conf` and `url.insteadOf`; mirror runs as an unprivileged host service, read-only to the guest, separate cache per trust domain (per tag) to bound poisoning. (b) `kiln cache warm <repo>`: boot, clone, install, snapshot, register as a pool shape (the pool and per-image warm default work already on the roadmap). Both reuse F3's allow list so only the mirror is reachable.
- **Effort and risk.** M, medium (poisoning, disk growth, eviction).
- **Verify.** Benchmark suite: cold `npm ci` of a fixed lockfile with and without the mirror; a poisoned package in one tag's cache is not visible from another tag; disk cap evicts.

### F7 SDK ergonomics

- **Want.** Retried creates must not double-create; agents want results as typed streams and a one-call "run this repo's tests".
- **Today.** `get-or-create` by name is idempotent; exec-stream logs over WebSocket; errors are typed in the MCP server ([sdks-and-cli.md](sdks-and-cli.md)). No idempotency key on plain create, no async iterator over exec-stream in the docs, no retry policy. (Evidence here is design reasoning, not a cited source.)
- **Design.** `Idempotency-Key` header on `POST /sandboxes` and `/snapshots/*/fork`, stored in sqlite for 24 h; SDK retry with jittered backoff on 502/503/connection reset for idempotent calls only; `for await (const chunk of sandbox.execStream(...))` and Python `async for`; helpers `sandbox.runTests({repo, command})` and `sandbox.cloneRepo()` as thin compositions of existing calls, kept in the SDKs, not the daemon.
- **Effort and risk.** S/M, low. **Verify.** Replay a create with one key, same id; kill the daemon connection mid-create and retry; both SDKs against the same topic.

### F8 Inbound access and stable preview URLs

- **Want.** Previews that a human or another service can open without the admin token. Self-hosted, token in a query string today. Evidence: the open-source platform routes sandbox URLs at an edge with "per-sandbox access tokens" ([infra README](https://raw.githubusercontent.com/e2b-dev/infra/main/README.md)); the Kubernetes sandbox project ships an optional router in front of sandboxes ([agent-sandbox](https://github.com/kubernetes-sigs/agent-sandbox)).
- **Today.** `/sandboxes/:id/preview/:port` accepts the one daemon token ([preview.md](preview.md)).
- **Design.** Per-sandbox preview token (`POST /sandboxes/:id/preview-token`, scoped to one id and port, expiring), optional wildcard host routing `<port>-<name>.preview.example` so cookies and absolute paths work. **Effort** S/M. **Verify.** A scoped token opens its port and gets 403 on another sandbox's; expired token refused; works after snapshot and resume.

### F9 Templates from a recipe

- **Want.** Declarative environment builds with layered, hashed, cached steps, then "create" is a restore of a pre-booted template; the open-source platform builds templates from container images and steps with a final pass recording touched pages ([infra README](https://raw.githubusercontent.com/e2b-dev/infra/main/README.md)); the Kubernetes project has `SandboxTemplate` and a warm pool of them ([agent-sandbox](https://github.com/kubernetes-sigs/agent-sandbox)).
- **Today.** Register a prebuilt ext4 (`POST /images`), `images/build-universal-image.sh`, pools of the default shape; OCI conversion is "not planned" ([images-and-auth.md](images-and-auth.md), [ideas-not-started.md](ideas-not-started.md)).
- **Gap.** Users must build an ext4 and inject the guest agent by hand.
- **Design (to be a separate note).** `sandkiln.toml` recipe (`from`, `run`, `copy`, `env`, `start_command`); `POST /templates` builds inside a sandbox (a build is just a sandbox, so it is isolated), snapshots the result, stores layer hashes; a template is image plus a baked snapshot plus a pool shape. Revisit the OCI "not planned" line: accepting `FROM` an OCI layout while building in a sandbox needs no host-side untrusted unpacking.
- **Effort and risk.** L, medium. **Verify.** Two builds with one changed step reuse the earlier layers (timed); create from template to first command vs today's 26 ms warm path; a corrupt recipe fails cleanly with no leaked lease.

### F10 to F16, briefly

| # | Evidence | Today | Sketch | Verify |
| --- | --- | --- | --- | --- |
| F10 Volumes | Workspace mounts and named volumes are the persistence model of an open-source agent runtime ([docs](https://docs.openhands.dev/openhands/usage/sandboxes/docker)); the Kubernetes project lists persistent storage as a core property ([README](https://github.com/kubernetes-sigs/agent-sandbox)) | Drives persist, share read-only ([drives-and-storage.md](drives-and-storage.md)) | `POST /drives/:id/clone` (reflink), size quota, `ro` snapshot of a drive | Clone diverges from parent; quota refused |
| F11 Audit and traces | The GenAI conventions define an `execute_tool` span and tool duration metric, still in development ([spec repo](https://github.com/open-telemetry/semantic-conventions-genai)) | Logs, request ids, `/metrics`, sqlite history ([observability.md](observability.md)) | Append-only audit table (who, route, sandbox), OTLP exporter behind a config flag, one span per exec | A trace shows create, exec, snapshot; audit row per mutating call |
| F12 Events | Kubernetes project wants auto-resume on network connection ([README](https://github.com/kubernetes-sigs/agent-sandbox)); MCP tasks notify on state change ([MCP tasks](https://modelcontextprotocol.io/extensions/tasks)) | Polling only | `GET /events` SSE of lifecycle, signed webhooks; guest inotify as an opt-in agent request | Webhook per transition, retried; no loss across daemon restart is **not** promised |
| F13 Integrations | Frameworks ship executors against a small interface ([AutoGen](https://microsoft.github.io/autogen/docs/topics/code-execution/jupyter-code-executor)) | JS and Python examples, MCP server | One adapter per framework once F5 exists, in `examples/` | Run the framework's own executor test against a live daemon |
| F14 Scoped keys | Weak evidence in open sources: the Kubernetes project lists dual user and sandbox identity as exploratory ([README](https://github.com/kubernetes-sigs/agent-sandbox)) | One bearer token, rate limits, ceilings ([images-and-auth.md](images-and-auth.md)) | Token table with scope (tags prefix, verbs), per-token quota, usage counters in `/metrics`; no billing | Token A cannot see B's sandboxes; quota returns 429 |
| F15 Browser, desktop | An open-source overlay bundles Chromium, Xvfb, x11vnc and noVNC as units ([Quicksand CUA](https://microsoft.github.io/quicksand/packages/quicksand-cua.html)); the Kubernetes project documents a computer-use use case ([page](https://agent-sandbox.sigs.k8s.io/docs/use-cases/computer-use/)) | Nothing; image and preview only | A published image plus CDP and noVNC reached through F5/F8 WebSocket preview; no new daemon API | Playwright `connect_over_cdp` loads a page; noVNC shows it |
| F16 Multi-node | Cluster-scale RL fleets exist ([AgentENV](https://kvcache.ai/blog/agentenv-open-sourced/), scale claims unverified) | One node | Unchanged from [ideas-not-started.md](ideas-not-started.md) | n/a |

## Explicitly not building

| Item | Why |
| --- | --- |
| GPU in a sandbox | The VMM maintainers say there is no known path ([issue 849](https://github.com/firecracker-microvm/firecracker/issues/849)); passthrough conflicts with memory overcommit ([issue 1104](https://github.com/firecracker-microvm/firecracker/issues/1104)); a proof-of-concept PCIe branch exists ([PR 4894](https://github.com/firecracker-microvm/firecracker/pull/4894)) but whether it ships is **unverified**. Revisit only if upstream releases it. A GPU job belongs in a different runtime (for example one built on Cloud Hypervisor or QEMU). |
| Git-native filesystem | A git server plus object store is a second product ([ideas-not-started.md](ideas-not-started.md)); F6 plus F9 plus snapshots give most of the benefit. |
| Durable workflow layer | Already out of scope; belongs in a library over the API. |
| Billing and payments | Metering counters (F14) yes; invoicing is a hosted-service concern and this is self-hosted. |
| TLS interception of all egress by default | Only hosts with a credential rule (F2) are terminated; blanket MITM breaks pinned clients and widens the trust surface. |
| Rewriting the guest agent as a Jupyter or LSP server | F5 reuses the kernel process; keep the agent small and static. |
| Full desktop environment as default image | Size and boot cost; ship as an optional image (F15). |

## What I could not verify

- Rich output (`display_data`) handling through the Jupyter gateway: no source found.
- Whether the Firecracker PCIe passthrough branch is merged or released.
- AgentENV and forkd numbers are the projects' own claims; scale (30,000 environments) has no data in the post. DeltaBox and Planarian are preprints, abstracts read only, not independently checked.
- "Hosted agent-sandbox services document SNI filtering with the domain-fronting caveat and a host-side proxy that resets after accept": seen in several vendor changelogs and blogs; not cited because the repo rule is not to name vendors. The open-source evidence above carries the same points.
- Firecracker's network-for-clones guide is read in full; the snapshot-support and `random-for-clones` pages were read as search excerpts only.
- Scoped-key demand (F14): no neutral source found; ranked low for that reason.
- The effort figures and the 1-5 value scores are this note's judgement, not measured.

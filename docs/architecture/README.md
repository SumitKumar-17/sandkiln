# sandkiln — architecture brief

This folder is a plain-markdown, GitHub-readable tour of the whole system: what's
built, why it's built that way, the technical terms/algorithms involved, and where
the real code is. It's deliberately **brief per topic**, not exhaustive — each file
is a few minutes of reading, aimed at someone who needs to explain this system
(an interview, a design review) without re-deriving it from source.

For a much deeper, example-driven version of most of these same topics — real
captured API request/response bytes, real headers, real measured numbers — see
[`website/src/content/docs/internals/`](../../website/src/content/docs/internals/),
also plain markdown, also readable directly on GitHub. This folder is the map;
that one is the terrain.

## What sandkiln is, in one paragraph

sandkiln boots a real Firecracker microVM per sandbox — its own kernel, its own
filesystem, its own network namespace — to run code nobody vouched for (an AI
agent's output, a user upload, anything untrusted). A Rust daemon
(`sandkilnd`) drives Firecracker directly over its own HTTP API, talks to a
small guest agent inside each VM over `AF_VSOCK`, and exposes the whole
lifecycle — create, exec, snapshot, resume, fork, drives, images, networking
policy — over its own HTTP API, wrapped by a JS/TS SDK, a Python SDK (sync and
async), and a CLI.

## The five Rust crates

| Crate | Owns | Depends on |
|---|---|---|
| `sandkiln-protocol` | The host↔guest wire format (length-prefixed JSON) | nothing |
| `sandkiln-guest-agent` | Static musl binary running inside every VM, answers vsock requests | protocol |
| `sandkiln-vmm` | Drives Firecracker directly: boot, snapshot/resume, networking, drives, the vsock client | protocol |
| `sandkiln-store` | sqlite-backed durable sandbox history | nothing |
| `sandkiln-daemon` | The axum HTTP API wrapping the whole lifecycle | protocol, vmm, store |

## Feature map

Each row links to the brief file covering it. "Status" is honest, not aspirational
— matches `ROADMAP.md`'s "What works today".

| Feature | Key terms / algorithms | Brief |
|---|---|---|
| HTTP API, auth, observability | bearer token, preview `?token=` fallback, request-id correlation, structured tracing | [01-daemon-api-and-auth.md](01-daemon-api-and-auth.md) |
| VM boot + startup latency | Firecracker API socket, rootfs clone (`cp --reflink=auto`), concurrent resource leasing | [02-vm-boot-and-latency.md](02-vm-boot-and-latency.md) |
| Pre-warmed pools, idle lifecycle | producer/consumer queue, `warm_count`/`max_count`, auto-suspend → archive tiers | [02-vm-boot-and-latency.md](02-vm-boot-and-latency.md) |
| Networking, egress policy | tap pool, Linux bridge, `CAP_NET_ADMIN` vs `TUNSETIFF`, iptables chains, MMDS | [03-networking-and-egress.md](03-networking-and-egress.md) |
| Snapshot / resume / fork | pause-and-snapshot, parent-pointer lineage DAG, retired checkpoints, time-travel restore | [04-snapshots-resume-fork.md](04-snapshots-resume-fork.md) |
| Drives, images, remote mounts | read-only sharing + ownership tracking, managed image registry, FUSE + rclone | [05-storage-drives-images-mounts.md](05-storage-drives-images-mounts.md) |
| vsock wire protocol | `AF_VSOCK`, length-prefixed framing, 4 connection shapes | [06-vsock-protocol.md](06-vsock-protocol.md) |
| Local tunnel | guest-initiated vsock, `<uds_path>_<port>` listener, WebSocket multiplexing frame | [12-local-tunnel.md](12-local-tunnel.md) |
| Security & privilege model | ambient capabilities, jailer (chroot/cgroups/setuid), path-traversal validation | [07-security-and-privilege.md](07-security-and-privilege.md) |
| Durable history (sqlite) | WAL journaling, `synchronous=NORMAL`, best-effort writes | [08-persistence-sqlite.md](08-persistence-sqlite.md) |
| PTY, exec-stream, rate limiting | `forkpty`, WebSocket relay, token-bucket algorithm | [09-interactive-sessions-and-rate-limits.md](09-interactive-sessions-and-rate-limits.md) |
| JS/Python SDKs, CLI | hand-rolled HTTP clients, zero runtime deps, sync/async split | [10-sdk-and-cli-design.md](10-sdk-and-cli-design.md) |
| Testing & benchmarking | parallel integration-test topics, criterion, load-test percentiles | [11-testing-and-benchmarking.md](11-testing-and-benchmarking.md) |

## How to use this for an interview-style walkthrough

A reasonable order to talk through the system out loud:

1. **The problem**: running untrusted/AI-generated code needs real isolation, not
   a container's shared kernel — [07](07-security-and-privilege.md) opens with this.
2. **The primitive**: a Firecracker microVM, booted and controlled by
   `sandkiln-vmm` — [02](02-vm-boot-and-latency.md).
3. **The control channel**: how the host talks to code running inside a VM with
   no network path at all — [06](06-vsock-protocol.md).
4. **The API surface**: what a caller actually sees — [01](01-daemon-api-and-auth.md),
   then whichever feature file matches what you want to go deep on.
5. **The hard problems that got found and fixed**: rootfs-clone dominating boot
   latency (not the network lease, as first suspected), the vsock timeout that
   could hang a stop forever, the DELETE-status-code bug — see each file's
   "Non-obvious things" section and `ROADMAP.md`'s full history for the rest.

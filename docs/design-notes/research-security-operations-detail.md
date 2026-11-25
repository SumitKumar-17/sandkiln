# Research: secrets, supply chain, audit and operations

Second half of [research-security-operations.md](research-security-operations.md), which holds the
current-state summary, sections 1 to 3, the ranking, the top five and the
rejected items. Same date (2026-11-25) and same convention: **unverified** marks
a claim not confirmed from a primary source or a real daemon.

## 4. Secrets and encryption at rest

**Secrets.** Firecracker's metadata store is not written into snapshots ("to
avoid leaking vm-specific information") and V2 needs a session token
([mmds](https://raw.githubusercontent.com/firecracker-microvm/firecracker/main/docs/mmds/mmds-user-guide.md)),
so a credential delivered through it is absent from the snapshot file itself.
Anything the guest then reads into process memory is in `mem.bin` (Firecracker:
snapshots hold "the full contents of guest memory"). So: (1) inject at exec
time as environment, never persist it in `sandbox_history`, request logs or
tags (check the store schema and the `TraceLayer` fields: **unverified**);
(2) offer per-sandbox secrets with an expiry delivered via the metadata store
and refreshed after resume; (3) the strongest design is an egress relay that
adds the credential to outbound requests so the secret never enters guest
memory (L, needs the domain-aware proxy first). Redaction of secret values in
daemon logs is a small, testable step (S).

**Encryption at rest.** The existing position stands: encrypt the filesystem,
not the files, because Firecracker writes `mem.bin` itself
([security-hardening.md](security-hardening.md)). Add: document dm-crypt/LUKS
for `state_dir`, `drives_dir`, `images_dir`, `archive_dir` and check it in
preflight (`lsblk`, S). Cost: block-layer AES-XTS is typically a few percent
CPU on AES-NI hosts and affects snapshot take and resume I/O; **no number
measured here**, so measure with `scripts/bench-report.sh` on an encrypted
volume before quoting any. Per-tenant keys (fscrypt per directory) fit
namespaces but add key lifecycle: defer. Because Firecracker only CRCs
snapshot files, add an integrity record: a SHA-256 in `meta.json` written at
take and checked at resume (M), which also detects disk corruption.

## 5. Supply chain

| Gap | Evidence | Change | Effort |
| --- | --- | --- | --- |
| Firecracker download unverified | `curl -L` then copy, no hash. The release publishes `*.tgz.sha256.txt` but no signature or provenance file (v1.16.2 asset list, API read 2026-11-25) | Commit the expected SHA-256 per version and arch in the repo; fail on mismatch. A checksum fetched from the same release only detects corruption | S |
| Kernel and rootfs from build scripts | No pinned hashes; published artifacts are planned (feature plan item 9) | Publish with SHA-256 and a [Sigstore](https://docs.sigstore.dev/) signature from CI; `setup.sh --download` verifies with `cosign verify-blob` | M |
| Provenance | GitHub's artifact attestations support SLSA build provenance and SBOM attestations ([docs](https://docs.github.com/actions/security-guides/using-artifact-attestations-to-establish-provenance-for-builds)) | Attest release binaries, the guest agent, the supervisor and images; SBOM in CycloneDX or SPDX | M |
| Image reproducibility | Image build installs packages at build time | Pin a distro snapshot date and package versions, record the lock file next to the image; true bit-for-bit reproducibility is a larger goal, **not claimed** | M |
| Dependencies | Lockfiles committed (finding 2), advisories fixed (finding 1) | Add `cargo audit` or `cargo deny` and `npm audit` to CI as failing checks | S |

Verify: tamper one byte of a cached tarball and confirm `setup.sh` refuses;
`cosign verify-blob` passes on a published artifact from a clean host.

## 6. Audit logging and forensics

Gap: request logs carry method, path, status, latency and a request id, but not
who (one token has no identity), and history records only create and end. Design:
an `audit_events` table in the history database beside `sandbox_history`
([persistence post](../../docs-site/engineering/09-persistence-and-history.md)):
time, token id, request id, action, target id, outcome, peer address. Never
bodies, env values or file contents. Events: auth failure, create, exec start
(command hash only), snapshot, resume, fork, stop, drive and image changes,
policy changes. Mirror to the JSON log so an external collector keeps a copy.
**Tamper evidence:** a hash chain where each row stores the hash of the
previous row plus its own fields, with the head periodically written to a file
the daemon cannot rewrite, gives detection of edits and deletions; a Merkle
tree gives cheap consistency proofs but is more than a single node needs
([Crosby and Wallach](https://static.usenix.org/events/sec09/tech/slides/crosby.pdf);
[RFC 6962](https://datatracker.ietf.org/doc/rfc6962), obsoleted by RFC 9162).
A chain on the same disk as the daemon proves nothing against a host root
attacker; the value is an externally shipped head. **Session recording** of PTY
and streamed exec: opt-in per sandbox, to a file in `state_dir`, with retention
by age; it captures secrets typed into terminals, so default off. Retention
setting for audit rows and recordings. Effort: audit table M, chain S on top,
recording M. Verify: perform a scripted sequence, confirm one row per action
with the right token id, edit a middle row and confirm `kiln audit verify`
reports the break.

## 7. Operations

**Observability.** Metrics exist (boot, exec, phase histograms,
`sandboxes_active`, `sandbox_vm_exits_total`, `pool_claims_total`,
`rootfs_clone_total`). Missing for alerting: free disk and tap pool as gauges
(they are in `GET /status`, not `/metrics`), request counts by status,
rejected-creates, and an `up`-able build-info gauge. OpenTelemetry traces would
tie a request id to boot and exec spans; the request-id plumbing is already
span-based, so an optional OTLP exporter behind a feature flag is M. Do not
replace the hand-rolled Prometheus text. Starter alert set (Prometheus, each
with a `for:` clause so a blip does not page; `absent()` for a vanished
target, see the [Robust Perception note](https://www.robustperception.io/absent-alerting-for-jobs/)):

| Alert | Expression sketch | For |
| --- | --- | --- |
| Daemon down | `up{job="sandkilnd"} == 0` | 2m |
| Metrics missing | `absent(up{job="sandkilnd"})` | 5m |
| Disk nearly full | free bytes of `state_dir` under 15 percent (needs the gauge) | 10m |
| Tap pool exhausted | free taps equal 0 (needs the gauge) | 2m |
| VMs dying | `rate(sandbox_vm_exits_total[10m])` above a baseline | 10m |
| Boot slow | p95 of `boot_duration_ms` above 3x the recorded baseline | 15m |
| Pool misses | `rate(pool_claims_total{result="cold_slot"}[15m])` high | 15m |

Ship them as `docs-site/self-hosting` rule file and test with `promtool test rules`.

**Backup and restore.** Persistent state is listed in operations.mdx. Add
`sandkilnd backup`: a consistent sqlite copy (`VACUUM INTO`, or the online backup
API) plus a manifest of `meta.json` files; snapshots are large and copied by
the operator (reflink copies are cheap on the XFS volume). Restore is the existing
startup reconcile. Verify: back up, wipe `state_dir`, restore, resume a
snapshot. Effort M.

**Upgrade and rolling restart.** SIGTERM snapshots live sandboxes (jailed ones
are destroyed); a crash orphans Firecracker. Snapshot format compatibility
across Firecracker versions is checked by Firecracker, and snapshots must resume
on an identical setup (snapshot-support doc above): so a Firecracker upgrade can
strand held snapshots. Record the Firecracker version in `meta.json` and refuse
or warn at resume (S). The Firecracker upgrade in section 1 needs this first.

**Packaging.** The systemd template lacks hardening that costs nothing:
`ProtectSystem=strict` with `ReadWritePaths`, `ProtectHome`, `PrivateTmp`,
`RestrictAddressFamilies`, `LockPersonality`, `SystemCallFilter`; check each
against the daemon's real paths with `systemd-analyze security` (S). A container
image of the daemon needs `/dev/kvm`, `/dev/net/tun`, `CAP_NET_ADMIN` and host
networking for the bridge and taps, which removes most of what a container is
for; low value now. deb and rpm packages wait for published guest artifacts.

**Multi-node.** Minimum design: a node registry (id, address, capacity, free
taps, free disk), a thin router that creates on the least-loaded node and
proxies by sandbox id, snapshot locality (a snapshot resumes only where its
files are, so route by recorded node; no migration), a per-node subnet so
bridges do not collide, and no overlay network (sandboxes are isolated from
each other anyway). Worth it when one box's measured ceiling (the planned load
test, [load-testing.md](load-testing.md)) or availability needs are exceeded;
not before. **Capacity planning:** memory is the first limit (sum of
`mem_size_mib` plus VMM overhead per VM, no overcommit when hostile tenants
share the host), then `tap_pool_size` and the sandbox IP pool, then disk for
checkpoints (cap `retained_checkpoints_max`, finding 24), then vCPU
oversubscription. Record the ceiling from the load test; none exists yet.

# Research: security, isolation and operations

Research note, written 2026-11-25. Question: what should sandkiln do next on
security, isolation and operations, in what order, and what should it refuse.
Sources were read on that date; a claim marked **unverified** is one I could not
confirm from a primary source or from a real daemon. Nothing here was built or
run; every "verify" line is a plan for a real daemon, not a result. Sections 4
to 7 (secrets and encryption, supply chain, audit, operations) are in
[research-security-operations-detail.md](research-security-operations-detail.md).

## What exists today

One bearer token for the whole API, compared in constant time
([auth.rs](../../core/crates/daemon/src/http/auth.rs)); the preview route also
takes that same token as `?token=`. Per-sandbox iptables egress chains keyed by
tap with CIDR allow and deny ([egress.rs](../../core/crates/vmm/src/egress.rs),
[egress-firewall.md](egress-firewall.md)). Bridge port isolation between taps.
Optional jailer through the supervisor design, cgroup v2 `memory.max` and
`cpu.max` only ([jailer/mod.rs](../../core/crates/vmm/src/jailer/mod.rs),
[security-hardening.md](security-hardening.md)); no `--new-pid-ns`, rlimits or
`pids.max` appear in the arguments. Firecracker's default seccomp filters (the
code never passes `--no-seccomp`; confirmed by grep, not live). Per-sandbox I/O
rate limiter, idle reaper, sqlite history, graceful SIGTERM that snapshots
sandboxes, Prometheus text `/metrics`, a systemd unit with
`NoNewPrivileges` and a one-capability bound. Not present: encryption at rest,
audit log, scopes, quotas on sandbox count, multi-node. The Firecracker install
script downloads a pinned `v1.16.1` tarball with `curl -L` and no checksum
([install-firecracker.sh](../../scripts/host-setup/install-firecracker.sh)).

## 1. Guest escape and VMM threat model

**Threat model.** Firecracker treats the guest as hostile from the first vCPU
thread; the layers are KVM, the VMM's own seccomp filters, then the jailer.
Its own documents say the defence in depth holds only if the production host
recommendations are followed:
[prod-host-setup.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/prod-host-setup.md),
[design.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/design.md)
(read 2026-11-25).

**Published advisories** (the project's own list shows exactly three, read
2026-11-25: <https://github.com/firecracker-microvm/firecracker/security/advisories>):

| Advisory | Affects | Condition | Sandkiln exposure |
| --- | --- | --- | --- |
| [CVE-2026-122192](https://github.com/firecracker-microvm/firecracker/security/advisories/GHSA-48ww-rqpp-2mgm), 2026-11-04, CVSS 7.5 | 0.23.0 to 1.16.1; fixed 1.16.2, 1.17.0 | Snapshot restored with the File memory backend and a balloon or virtio-mem device, plus host memory pressure; guest root reads or writes VMM memory | Pinned version is affected. The vmm sets no balloon (grep found none), so the precondition is absent; **unverified live**. Still behind by two minor releases |
| [CVE-2026-5747](https://github.com/firecracker-microvm/firecracker/security/advisories/GHSA-776c-mpj7-jm3r), 2026-04-07, CVSS 8.7 | 1.13.0 to 1.14.3, 1.15.0 | Only with `--enable-pci`; the default MMIO transport is not affected | Not affected: kernel args use `pci=off` and the flag is not passed |
| [CVE-2026-1386](https://github.com/firecracker-microvm/firecracker/security/advisories/GHSA-36j2-f825-qvgc), 2026-01-23, moderate | up to 1.13.1, 1.14.0 | Jailer run as root follows symlinks in pre-created directories | Fixed in the pinned version; shows the jailer trusts its inputs, so `chroot_base_dir` must stay unwritable by others |

Latest release at the time of writing is v1.18.0 (2026-11-09); v1.16.2 and
v1.17.0 shipped 2026-09-10 (GitHub releases API, read 2026-11-25).

**Recommended host setup** (prod-host-setup.md): keep the default seccomp
filters ("Production usage of the `--seccomp-filter` or `--no-seccomp`
parameters is not recommended"); run through the jailer with a unique
unprivileged uid per VM; cgroup limits on memory, CPU and disk; disable SMT
"in production scenarios that require tenant separation"; disable KSM; disable
host swap or use secure swap; one tenant per Firecracker process; ECC memory;
keep microcode and kernels patched; bound serial and log output; run a
watchdog that kills hung Firecracker processes; filter guest egress on the
host, naming the metadata address.

**Side channels.** The kernel's L1TF document says an L1D flush does not
stop a sibling hyperthread from refilling the cache, and offers core-affine
exclusive cpusets or disabling SMT
([l1tf](https://docs.kernel.org/6.8/admin-guide/hw-vuln/l1tf.html)). Firecracker's CPU
templates "shall not be used as a security protection against malicious
guests" and static templates are deprecated
([cpu-templates](https://raw.githubusercontent.com/firecracker-microvm/firecracker/v1.16.1/docs/cpu_templates/cpu-templates.md)),
so do not offer them as a hardening knob.

**Nested virtualization.** No official Firecracker policy found; a maintainer
thread calls it out of scope and not for production
([issue 1721](https://github.com/firecracker-microvm/firecracker/issues/1721),
community, not policy). Kernel docs note `nested` is on by default on x86 since
4.20 ([running-nested-guests](https://docs.kernel.org/virt/kvm/x86/running-nested-guests.html)).
Upstream's own guide says its tests use only bare-metal hosts. Position: a
nested host is acceptable for development and CI, and the first-level
hypervisor joins the attack surface for hostile tenants.

**What sandkiln's defaults leave open.**
1. Jailer off by default: no chroot, no uid separation, no cgroup, so a VMM
   escape lands as the daemon's user.
2. Jailed mode omits `--new-pid-ns`, `fsize`, `no-file` limits and `pids.max`
   (jailer docs: [jailer.md](https://github.com/firecracker-microvm/firecracker/blob/main/docs/jailer.md)).
3. Nothing checks SMT, KSM, swap, mitigations or microcode; preflight is silent.
4. Snapshot clones of one memory image: Firecracker calls resuming the same
   state more than once insecure for duplicated secrets and tokens, and the
   kernel PRNG reseed (VMGenID) needs a guest Linux 5.18 or later
   ([snapshot-support](https://raw.githubusercontent.com/firecracker-microvm/firecracker/main/docs/snapshotting/snapshot-support.md)).
   Finding 28 covered entropy and clock; application-level secrets in forks
   are an undocumented residual. Guest kernel version in the image: **unverified**.
5. Snapshot files have only a 64-bit CRC, "a partial measure ... against
   accidental corruption"; a host user who can write `state_dir` can alter a
   guest's memory (same source).

**Change A, S.** Move the default to the patched release (v1.18.0, or v1.16.2
at minimum), pin its SHA-256 in the repo and verify it in
`install-firecracker.sh` (see section 5). **Change B, S to M.** Add a
`--production-host` check group to `preflight-check.sh`: read
`/sys/devices/system/cpu/smt/active`, `/sys/kernel/mm/ksm/run`, `/proc/swaps`
and `/sys/devices/system/cpu/vulnerabilities/*`, and warn on SMT on, KSM on,
swap on, jailer off; surface the same in `GET /status`. **Change C, M.** Jailer
completion: `--new-pid-ns`, `--resource-limit fsize=` (also bounds snapshot
file size) and `no-file=`, `pids.max` in the cgroup, then jailed snapshots
(roadmap item 3). Verify: `kiln doctor` lists each check; a jailed sandbox's
`/proc/<pid>/status` shows the new uid, a fork bomb in the guest cannot raise
the host process count, and a `kill -9` of the daemon still ends the VMs.

## 2. Network isolation depth

**Gap 1: the default reaches the LAN, the host and cloud metadata.** Egress is
`allow_all` unless a caller sets a policy, and the only bridge-wide rule is
accept toward the uplink plus masquerade
([network.rs](../../core/crates/vmm/src/network.rs)). Firecracker's network
guide states the guest uses the tap IP as gateway and can reach the host there,
and that its quick-start rules do not restrict this
([network-setup](https://raw.githubusercontent.com/firecracker-microvm/firecracker/main/docs/network-setup.md)).
I found no `INPUT` rule in `network.rs` or `egress.rs`; whether a guest can
reach host services bound to `0.0.0.0` is **unverified on a live box**. On a
cloud host the link-local metadata address is reachable through the uplink
unless blocked; the Firecracker production guide names this address.
Firecracker's own metadata service lives at the same default address inside the
guest and intercepts it, so a daemon-level block applies only to the uplink path
([mmds](https://raw.githubusercontent.com/firecracker-microvm/firecracker/main/docs/mmds/mmds-user-guide.md)).
**Change, S to M:** a daemon-wide always-on deny set applied before any
per-sandbox rule: `169.254.0.0/16`, the host's own addresses, and the bridge
subnet except the gateway's DNS (udp and tcp 53); operator-configurable
`egress_hard_deny_cidrs` to add RFC 1918 (default off, because the dev box and
tests rely on LAN reach); an `INPUT` rule from tap devices accepting only DNS.
Also drop outbound tcp 25 by default. Verify: from a guest, `curl` to the host's
bridge address on a non-DNS port and to `169.254.169.254` must fail, and a
`deny_cidrs`-free sandbox still resolves names and reaches the internet.

**Gap 2: domain allow lists.** Options, with the failure mode of each:

| Approach | Strength | Failure mode |
| --- | --- | --- |
| DNS filtering only | Cheap, protocol-agnostic | Hard-coded IPs and other resolvers bypass it; DoT port 853 and DoH must be blocked |
| nftables sets filled from DNS answers | Packet-level default deny, works for any TCP port | Shared CDN addresses over-allow; entries need bounded timeouts. dnsmasq `--nftset` adds answers to an existing set; its manual does not state a timeout behaviour (**unverified**), and a set is global, not per source ([dnsmasq man](https://thekelleys.org.uk/dnsmasq/docs/dnsmasq-man.html)) |
| SNI or Host check in a relay | Closes the CDN over-allow for 80 and 443 | Client-set fields: domain fronting works without TLS interception, Encrypted Client Hello hides the name |
| TLS-terminating proxy | Full request visibility | Per-sandbox CA, breaks pinning; already listed as a stretch goal |

The open-source project [sandwall](https://github.com/anvanster/sandwall)
(Apache-2.0, read 2026-11-25) combines the first three on Firecracker: a
per-sandbox DNS proxy fills a timed nftables set before answering, non-listed
names get REFUSED, DoT is dropped, QUIC to learned addresses is rejected, and it
documents the residuals above, including DNS-query exfiltration to an allowed
zone. Recommended order for sandkiln: (1) port rules and an nftables migration
of the chain (the egress note already marks ports as a straightforward
extension); (2) a source-aware DNS proxy in the daemon that learns IPs into
per-sandbox nft sets with TTL timeouts and refuses unlisted names; (3) SNI/Host
check only if a user needs it. Effort L; do it after the hard blocks.
Verify: `deny_all` plus `allow_domains=["example.org"]` reaches that name,
fails another name, fails a raw IP, fails DoT, and the set empties after the
TTL.

**Gap 3: inbound.** The daemon listens on `127.0.0.1:7777` by default (good) and
has no TLS. The preview route accepts the long-lived master token as
`?token=` ([auth.rs](../../core/crates/daemon/src/http/auth.rs)), which lands in
proxy logs, browser history and referrers. **Change, S to M:** short-lived,
sandbox-scoped signed preview URLs (HMAC over sandbox id, port and expiry);
keep the header form. Terminate TLS in a reverse proxy or a Unix socket.

**Abuse.** Mining detection by CPU alone is evaded by throttled miners;
network signals (pool ports and names) plus sustained use are sturdier
(**secondary sources, not cited**). Cheap controls that match sandkiln: a
connection-rate and concurrent-connection limit per tap in the same chain (nft
`ct count`, `limit rate`, **unverified on this kernel**); the existing NIC rate
limiter for bandwidth; a hard cap on total sandboxes and on lifetime (below);
cgroup `cpu.max` already exists under the jailer. Port-scan abuse is bounded by
`deny_all` defaults for untrusted callers. **Per-sandbox DNS:** the single
dnsmasq cannot differentiate sources; the proxy above replaces it for policed
sandboxes only.

## 3. Authentication and multi-tenancy

Today any token holder can see and act on everything, including snapshots,
drives and images (`ROADMAP.md`: "Per-token scoping planned"). Gaps and design:

| Need | Design fitted to this codebase | Effort |
| --- | --- | --- |
| Several named tokens, rotation | `auth_tokens_file` (TOML: id, SHA-256 of secret, scopes, namespace, expiry); reload on SIGHUP; keep `SANDKILN_AUTH_TOKEN` as a single admin entry. Store hashes, compare in constant time. Rotation is add, deploy, remove | M |
| Scopes | `read` (GET), `exec`, `write` (create, snapshot, drives), `admin` (images, pools, config). One middleware maps method and route to a required scope; table-driven so it is unit-testable like `token_matches` | M |
| Namespaces (tenancy) | `owner` column on sandbox, snapshot, drive, image, history rows; list and get filter by the token's namespace; cross-namespace is 404, not 403. `drive_holder()` keeps double-attach protection. Fork and resume inherit owner | L |
| Quotas | `max_sandboxes` total (no such setting exists in the registry today), then per-namespace sandbox count, vCPU and memory sums, drive bytes. Reject at create with 429 and a metric | S for global, M per tenant |
| Metering | Already have `sandbox_history`; add owner, vcpu, memory, wall seconds. A `GET /usage` aggregate | M |
| mTLS, signed requests | Reject in the daemon; use a reverse proxy for mTLS. A bearer secret over TLS is adequate for a self-hosted daemon | Reject |

Verify (namespaces): token A creates a sandbox and snapshot; token B gets 404
on both, cannot list them, cannot attach A's drive, and the exact-id-set pattern
from `18-pool.sh` proves it under the concurrent suite.

## Ranking

Score is value (risk reduced or capability unlocked, 1 to 5) divided by effort
(S=1, M=2, L=4), rounded. The "Do next" list below groups ranks 2 and 3, and
pulls rank 6 ahead of equal scores because it unblocks ranks 7 to 10.

| Rank | Item | Value | Effort | Score |
| --- | --- | --- | --- | --- |
| 1 | Firecracker to patched release, pinned SHA-256 in the install script | 5 | S | 5.0 |
| 2 | Production-host preflight (SMT, KSM, swap, jailer on) and `/status` surface | 4 | S | 4.0 |
| 3 | Global `max_sandboxes`, max lifetime, snapshot Firecracker-version record | 4 | S | 4.0 |
| 4 | Hard-deny set (link-local, host, bridge) and tap `INPUT` lock, drop tcp 25 | 5 | S to M | 3.3 |
| 5 | Alert rules, alerting gauges, systemd hardening | 3 | S | 3.0 |
| 6 | Named tokens with ids and SIGHUP reload, then audit table with hash chain | 4 | M | 2.0 |
| 7 | Signed short-lived preview URLs | 3 | S to M | 2.0 |
| 8 | Jailer completion (pid ns, rlimits, `pids.max`, jailed snapshots) | 4 | M | 2.0 |
| 9 | Snapshot integrity hash, published signed artifacts, SBOM | 3 | M | 1.5 |
| 10 | Scopes, then namespaces, quotas and metering | 5 | L | 1.2 |
| 11 | Port rules, then DNS-learned domain allow lists | 4 | L | 1.0 |
| 12 | Backup command, OTel traces, session recording | 2 | M | 1.0 |
| 13 | Egress credential relay, per-tenant disk keys | 3 | L | 0.8 |
| 14 | Multi-node | 2 | L | 0.5 |

## Do next, in order

1. **Patched Firecracker with a pinned hash.** Highest evidence: the pinned
   `v1.16.1` is inside the affected range of a high-severity guest-to-VMM memory
   advisory published 2026-11-04, the fix has been out since 2026-09-10, and
   the download is unverified. Needs the version-in-metadata check first so
   held snapshots are not silently stranded. Verify with the full suite and
   `scripts/restart-test.sh`.
2. **Production-host preflight plus caps.** Cheap, and turns the upstream
   guide's recommendations (SMT, KSM, swap, jailer) from folklore into a
   visible check; a global sandbox cap is the missing resource ceiling.
3. **Hard-deny network defaults.** The default allows reaching the LAN and
   likely the host; Firecracker's own guide names the metadata address as a
   thing the operator must block. Do it before any user exposes a daemon
   to untrusted code. Live-verify the host-reach claim first.
4. **Alerts, gauges and unit hardening.** Small, and everything after it
   (upgrades, caps) needs visibility to be safe.
5. **Named tokens and the audit table.** The prerequisite for scopes,
   tenancy, quotas and metering, and the only way to answer who did what.
   Scopes and namespaces follow once more than one caller exists.

## Reject

- **Application-level snapshot encryption.** Firecracker writes the files
  itself; filesystem encryption covers the threat (already decided).
- **mTLS or request signing in the daemon.** A reverse proxy does it better;
  a bearer over TLS suits a self-hosted daemon.
- **Firecracker CPU templates as hardening.** Upstream says they are not a
  security boundary.
- **TLS-terminating inspection proxy now.** Per-sandbox CA and breakage cost
  far exceed the gain over domain filtering.
- **Daemon container image with KVM passthrough, deb and rpm now.** Host
  networking and device needs erase the benefit; no published artifacts yet.
- **Multi-node before a measured ceiling or an availability need.**
- **OTel metrics replacing `/metrics`.** Adds a dependency for no new signal.
- **Custom seccomp filters or `--no-seccomp`.** Upstream advises against both
  in production; keep the defaults.

## Not verified

Live reachability of host services from a guest; guest kernel VMGenID
support; whether the history store or logs ever hold exec environment values;
dnsmasq set timeout behaviour; `ct count` support on the dev kernel; the cost
of dm-crypt on snapshot take and resume; whether the Firecracker advisory
precondition (balloon, File backend) can arise through any sandkiln path;
cryptominer evasion claims (secondary sources only).

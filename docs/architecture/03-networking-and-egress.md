# Networking, isolation, and egress policy

## The primitive

Every sandbox gets its own **tap device**, leased from a pre-created pool,
attached to a shared Linux bridge — `sandkiln-vmm::network`. That's what gives
each sandbox a real, distinct network namespace rather than a shared one.

## Why a pre-created tap *pool*, not on-demand tap creation

The daemon runs unprivileged with exactly one Linux capability raised into its
ambient set: `CAP_NET_ADMIN`. That capability covers **netlink** operations
(attaching a tap device to the bridge, bringing a link up/down) but **not**
creating a brand-new tap device, which goes through a different kernel path
(`TUNSETIFF` on `/dev/net/tun`) that doesn't honor ambient `CAP_NET_ADMIN` the
way netlink calls do. So tap devices are pre-created once, as real root, by
`scripts/host-setup/create-tap-pool.sh`; the daemon only ever leases an existing
device from that pool. Trade-off: a fixed pool size caps concurrent sandboxes
until it's grown.

## Isolation

- **Bridge port isolation** stops sandbox-to-sandbox traffic at the network
  layer — verified live: two sandboxes on the same bridge can't reach each
  other directly, while both still reach the gateway and the internet.
- **Outbound**: NAT through the bridge, DNS through a host-local proxy
  (`scripts/host-setup/start-dns-proxy.sh`) — no per-sandbox config needed.

## Egress (outbound) policy — per-sandbox firewall

Optional `egress` field on `POST /sandboxes`: `{ mode, allow_cidrs, deny_cidrs }`.

- `mode: allow_all` (default) or `deny_all`.
- **One dedicated iptables chain per sandbox**, named from its tap device, with
  a single jump rule ahead of the daemon's own bridge-wide rule — so only that
  sandbox's traffic is affected.
- **`deny_cidrs` always wins on overlap.** iptables evaluates a chain top to
  bottom, first match wins; every deny rule is placed before every allow rule —
  no special-casing needed to make deny authoritative.
- **DNS is structurally exempt**, not allowlisted: rules only match traffic
  leaving through the daemon's own uplink interface, and DNS queries to the
  bridge's own gateway IP never transit that uplink at all. Even a `deny_all`
  sandbox with nothing allowed can still resolve names.
- Policy lifecycle is tied to the **network lease**, not the VM: applied when
  the lease goes live (boot, pool claim, resume, fork, time-travel restore),
  removed only when the lease is released (destroy, or deleting a held
  snapshot/checkpoint). A plain stop-and-preserve leaves the chain dormant but
  intact, like the tap device itself.
- Exposed in both SDKs and the CLI (`--egress-mode`/`--allow-cidr`/`--deny-cidr`).
- **Not built**: domain-level rules (would need the shared DNS proxy to become
  source-IP-aware) and port-level matching (`-p tcp --dport`) — a real gap, not
  a hidden one.
- **A real finding worth knowing**: the project's own dev box has *no* outbound
  internet route at all at the host level — egress live-testing had to use a
  real LAN address (the gateway), not a public IP, and the example project
  (`examples/egress-policy`) takes its target IP from an env var for exactly
  this reason, instead of hardcoding a public address that would silently pass
  with no real route.

## MMDS (guest-accessible metadata)

Every sandbox created with a name/tags serves its own `{id, name, tags}` at
`http://169.254.169.254/` — Firecracker's own native MMDS, answered by
Firecracker's device model directly, not the guest agent. Configured **V2**
(token-gated): a `PUT .../api/token` first, then the token as
`X-metadata-token` — specifically because a sandbox may run untrusted code that
shouldn't read metadata via an unauthenticated V1-style request.

## Status

Done, live-verified (`scripts/integration-test.sh`'s `10-jailer.sh`/`15-guest-metadata.sh`/`19-egress.sh`).
See [`website/src/content/docs/internals/tap-bridge-networking.md`](../../website/src/content/docs/internals/tap-bridge-networking.md)
and [`egress-iptables.md`](../../website/src/content/docs/internals/egress-iptables.md) for
real captured `ping`/iptables output.

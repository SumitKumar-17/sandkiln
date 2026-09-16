---
title: Networking & isolation
description: How sandboxes reach the internet, and why they can't reach each other.
---

Every sandbox gets its own tap device, leased from a pre-created pool, attached to a shared Linux bridge. That's what gives it a real network namespace distinct from every other sandbox on the host.

## Outbound access

Sandboxes NAT outbound through the bridge and resolve DNS through a host-local DNS proxy (`scripts/host-setup/start-dns-proxy.sh`) — no per-sandbox network configuration needed, it just works the way a normal Linux box with internet access would.

## Sandbox-to-sandbox isolation

Bridge port isolation means sandboxes can't reach each other directly — verified live: two sandboxes on the same bridge, neither can open a connection to the other, while both can still reach the gateway and the outbound internet. This is the network-layer half of isolation; the kernel/hardware half is the microVM boundary itself (see [The jailer and privilege model](../../internals/jailer-privilege-model/)).

## Why a pre-created tap pool

The daemon runs unprivileged, with exactly one Linux capability raised — `CAP_NET_ADMIN`, ambient, not full root. That capability covers netlink operations (attaching a tap device to the bridge, bringing a link up or down) but **not** creating a brand-new tap device, which goes through a different kernel path (`TUNSETIFF` on `/dev/net/tun`) that doesn't honor ambient `CAP_NET_ADMIN` the way netlink calls do. So tap devices are pre-created once, as real root, by a one-time setup script (`scripts/host-setup/create-tap-pool.sh`, or `scripts/setup.sh` end to end) — the daemon only ever leases an existing device from that pool. See [TAP devices and bridge networking](../../internals/tap-bridge-networking/) for the full reasoning, including the trade-off this creates (a fixed pool size caps concurrent sandboxes until it's grown).

## Guest-accessible metadata (MMDS)

Every sandbox created with a name and/or tags automatically serves its own `{id, name, tags}` to itself at `http://169.254.169.254/` — Firecracker's own native MMDS (Microvm Metadata Service), a link-local endpoint answered by Firecracker's device model directly, not by the guest agent. Nothing to configure: no sandkiln HTTP route or SDK method is involved on the read side at all, since it's entirely inside the guest's own network stack.

Configured V2 (token-gated), specifically because a sandbox may run untrusted or AI-generated code that shouldn't be able to read metadata via an unauthenticated request the way V1 would allow:

```bash
# Run this from inside the sandbox (e.g. via exec):
TOKEN=$(curl -s -X PUT http://169.254.169.254/latest/api/token \
  -H "X-metadata-token-ttl-seconds: 21600")
curl -s -H "X-metadata-token: $TOKEN" -H "Accept: application/json" http://169.254.169.254/
# {"id":"...","name":"build-worker","tags":{"env":"ci"}}
```

`Accept: application/json` matters — a `GET` with no `Accept` header returns an AWS-IMDS-style newline-separated list of top-level key names instead of the actual value, a real Firecracker behavior worth knowing about, not a sandkiln bug.

## Egress (outbound network) policy

By default every sandbox keeps unrestricted outbound access — the behavior above, unchanged. An optional `egress` field on `POST /sandboxes` (and `POST /sandboxes/get-or-create`) restricts it per sandbox:

```json
{
  "egress": {
    "mode": "deny_all",
    "allow_cidrs": ["10.0.0.0/8"],
    "deny_cidrs": []
  }
}
```

`mode` is `allow_all` (today's default-open behavior, minus whatever `deny_cidrs` subtracts from it) or `deny_all` (nothing outbound except what `allow_cidrs` opens back up). Enforced with one dedicated iptables chain per sandbox, named from its tap device, with a single jump rule inserted ahead of the daemon's own bridge-wide rule so only that sandbox's traffic is affected. `deny_cidrs` rules always win over `allow_cidrs` on overlap — iptables evaluates a chain top to bottom, first match wins, and every deny rule is placed before every allow rule, so there's no special-casing needed to make deny authoritative.

Rules match only traffic actually leaving through the daemon's own uplink interface, the same scoping the bridge-wide rule already uses — which has a useful side effect: DNS queries to the bridge's own gateway IP never transit the uplink at all, so they're structurally exempt from any egress policy without needing an explicit allowlist entry. Even a `deny_all` sandbox with nothing allowed can still resolve names; it just can't reach anything past the gateway that isn't explicitly permitted.

A policy is tied to the sandbox's network lease, not its VM: applied once the lease goes live (fresh boot, pool claim, resume, fork, or a [time-travel restore](../snapshots/)) and removed only when the lease is finally released (a full destroy, or deleting a held snapshot or retired checkpoint) — a plain stop-and-preserve leaves the chain dormant but intact, exactly like the tap device it's attached to.

**Not yet built**: domain-level rules (would need the shared DNS proxy, currently one instance with no per-source differentiation, to become source-IP-aware — a substantially bigger change) and port-level matching (`-p tcp --dport`, a straightforward extension of the same rule shape, just not built yet). IPv4 only, matching every other networking type in this project. No SDK/CLI surface yet — daemon HTTP API only, for now.

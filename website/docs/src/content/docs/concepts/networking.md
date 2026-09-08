---
title: Networking & isolation
description: How sandboxes reach the internet, and why they can't reach each other.
---

Every sandbox gets its own tap device, leased from a pre-created pool, attached to a shared Linux bridge. That's what gives it a real network namespace distinct from every other sandbox on the host.

## Outbound access

Sandboxes NAT outbound through the bridge and resolve DNS through a host-local DNS proxy (`scripts/host-setup/start-dns-proxy.sh`) — no per-sandbox network configuration needed, it just works the way a normal Linux box with internet access would.

## Sandbox-to-sandbox isolation

Bridge port isolation means sandboxes can't reach each other directly — verified live: two sandboxes on the same bridge, neither can open a connection to the other, while both can still reach the gateway and the outbound internet. This is the network-layer half of isolation; the kernel/hardware half is the microVM boundary itself (see [Privilege model](../../architecture/privilege-model/)).

## Why a pre-created tap pool

The daemon runs unprivileged, with exactly one Linux capability raised — `CAP_NET_ADMIN`, ambient, not full root. That capability covers netlink operations (attaching a tap device to the bridge, bringing a link up or down) but **not** creating a brand-new tap device, which goes through a different kernel path (`TUNSETIFF` on `/dev/net/tun`) that doesn't honor ambient `CAP_NET_ADMIN` the way netlink calls do. So tap devices are pre-created once, as real root, by a one-time setup script (`scripts/host-setup/create-tap-pool.sh`, or `scripts/setup.sh` end to end) — the daemon only ever leases an existing device from that pool. See [Privilege model](../../architecture/privilege-model/) for the full reasoning, including the trade-off this creates (a fixed pool size caps concurrent sandboxes until it's grown).

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

## What's not done yet

There's no per-sandbox egress policy yet — every sandbox has full outbound access by default. A domain/IP/port allow-deny policy, enforced at the DNS proxy layer before a connection ever opens, is on the project's Roadmap page but not implemented.

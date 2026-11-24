# sandkiln egress policy

Per-sandbox outbound policy: `deny_all` blocks an unlisted destination and `allowCidrs` opens it again.

## What it does

1. Creates a sandbox with no policy and pings `EGRESS_EXAMPLE_TARGET_IP`. It must succeed; otherwise the example stops, since nothing later could prove anything.
2. Creates a sandbox with `egress: { mode: "deny_all" }` and pings the same target. It must fail.
3. Creates a sandbox with `deny_all` plus `allowCidrs: ["<target>/32"]` and pings again. It must succeed.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service.

## Run it

```
cd examples/egress-policy
npm install
EGRESS_EXAMPLE_TARGET_IP=<ip> node index.js
```

## Configuration

- `EGRESS_EXAMPLE_TARGET_IP` (required): an address reachable from the daemon host, such as its default gateway (`ip route`). It is not hardcoded because a fixed public IP would pass falsely on a host with no outbound route, where `deny_all` and no policy both just time out.
- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- IP and CIDR rules only; no domain or port matching yet (see `ROADMAP.md`).
- The daemon re-applies a policy on resume and fork; this script does nothing for that.

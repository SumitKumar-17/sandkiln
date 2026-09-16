# sandkiln egress policy

A minimal reference example of per-sandbox outbound network policy:
`deny_all` blocking an unlisted destination, `allow_cidrs` opening it
back up, and (implicitly, by not needing an allowlist entry to still
resolve names) gateway-bound traffic staying reachable regardless of
policy.

> **Why this needs a target IP you supply, not a hardcoded one**: a
> hardcoded public IP would silently "pass" this example on a machine
> with no outbound internet route at all (confirmed while building this:
> not every dev/CI box has one, and `deny_all` and no-policy-at-all look
> identical from *that* kind of failure — both time out). A hardcoded LAN
> IP isn't portable across networks either. The one thing this example
> can rely on is whatever address *you* already know your own daemon
> host can reach — its own default gateway is usually a safe choice.

## What it does

1. Creates a sandbox with **no egress policy** (today's default:
   unrestricted outbound) and pings `EGRESS_EXAMPLE_TARGET_IP` — this
   should succeed. If it doesn't, the target itself isn't reachable from
   this network and the rest of the example can't prove anything, so it
   stops here with a clear message rather than reporting a false
   "blocked" a few steps later.
2. Creates a sandbox with `egress: { mode: "deny_all" }` and no
   `allowCidrs`, and pings the same target — this should fail. `deny_all`
   blocks everything not explicitly allowed.
3. Creates a sandbox with `egress: { mode: "deny_all", allowCidrs:
   ["<target>/32"] }` and pings again — this should succeed. `allow_cidrs`
   opens specific destinations back up under an otherwise-closed policy.

Each sandbox is destroyed (`stop({ keep: false })`) once its check is
done. See `index.js` — it's the whole program.

## Requirements

A running `sandkilnd` daemon — see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the repo root. There is no
hosted service.

## Configuration

- `EGRESS_EXAMPLE_TARGET_IP` — a real IP address reachable from the
  daemon host's own network (its default gateway is a reasonable
  choice — find it with `ip route` on the daemon host). Required.
- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one.

## Known limitations

- Only IP/CIDR matching — no domain-level or port-level rules yet (see
  `ROADMAP.md`'s "Firewall and egress policy" section for what's
  deferred and why).
- A policy is tied to a sandbox's lease, not carried over on resume/fork
  by any code in this example — it's re-applied automatically by the
  daemon in both cases, so nothing extra is needed here, but that's the
  daemon's behavior, not this script's.

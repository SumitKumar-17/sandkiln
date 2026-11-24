# Firewall and egress policy

## Done: per-sandbox IP/CIDR egress policy

`egress: { mode, allow_cidrs, deny_cidrs }` on `POST /sandboxes` and
`POST /sandboxes/get-or-create`. `mode` is `allow_all` (the default, minus
whatever `deny_cidrs` subtracts) or `deny_all` (nothing outbound except
`allow_cidrs`).

How it is enforced (`sandkiln_vmm::egress`):
- One dedicated iptables chain per sandbox, named from its tap device, with
  a single jump rule inserted ahead of the bridge-wide `FORWARD ACCEPT` so
  only that sandbox is affected.
- Within the chain: all `deny_cidrs`, then all `allow_cidrs`, then the base
  mode's default. First match wins, so deny beats allow on overlap with no
  special-casing.
- Rules match only traffic leaving via the uplink interface. Gateway-bound
  traffic (DNS to the bridge IP) never transits the uplink, so it is
  structurally exempt: even `deny_all` with an empty allowlist can still
  resolve names but reaches nothing past the gateway.
- A policy is tied to the lease, not the VM. Applied when a lease goes live
  (boot, pool claim, resume, fork), removed when the lease is released
  (destroy or deleting a held snapshot). A snapshot-and-stop leaves the
  chain dormant but intact, like the tap.
- Persists through snapshot, resume and fork, re-applied from the
  snapshot's retained policy each time (iptables state does not survive a
  host reboot). A re-apply failure on resume or fork is a loud warning, not
  fatal: both are hard to redo, and this is a best-effort hardening layer,
  the same framing as the jailer.

Verified live: `deny_all` blocks an unlisted LAN destination; an
`allow_cidrs` entry reopens it; a `deny_cidrs` entry blocks one destination
under `allow_all`; gateway traffic stays reachable; the policy survives
snapshot, resume and fork and is torn down on destroy.
`scripts/integration-tests/19-egress.sh` covers the host-agnostic subset in
CI (validation, normal use, survival across snapshot/resume/fork). The
allow/deny/overlap behavior needs a second reachable LAN address and is
manually verified only. The dev box turned out to have no outbound internet
route, so a LAN address was used.

SDK and CLI: `Sandbox.create({ egress })` / `getOrCreate({ egress })`
(JS/TS), `egress_mode`/`egress_allow_cidrs`/`egress_deny_cidrs` kwargs
(Python), `--egress-mode <allow_all|deny_all>` with repeatable
`--allow-cidr`/`--deny-cidr` (CLI). Reference project:
`examples/egress-policy`.

## Hot-path fix

`egress::apply` spawned one `iptables` per rule (a 6-CIDR policy meant 9
spawns, about 8.3ms per create). It now loads one `iptables-restore
--noflush`; only the two shared-`FORWARD` rules remain separate calls (a
`-C` check, and `-I` only when missing). Measured 8.3ms to 3.1ms (-63%),
over 17 fresh creates. A new `egress_apply` create-phase metric (recorded
only when a policy is requested) makes the cost visible.

## Deferred

- Domain-level rules: the shared DNS proxy (one `dnsmasq`, no per-source
  differentiation) would have to become source-IP-aware.
- Port-level matching (`-p tcp --dport`): a straightforward extension of
  the same rule shape, not built.
- IPv4 only, like every other networking type here.
- Request-level matchers (path, method, query, header) that forward or
  transform, useful for a proxy in front of a sandbox's exposed port.
- A per-sandbox CA plus TLS-terminating proxy for HTTPS inspection, mounted
  into the guest trust store at boot. Much more complex than IP or domain
  filtering; a stretch goal.

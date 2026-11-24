# Firewall and egress policy

## Done: per-sandbox IP/CIDR egress policy

`egress: { mode, allow_cidrs, deny_cidrs }` on `POST /sandboxes` and
`POST /sandboxes/get-or-create`. `mode` is `allow_all` (the default, minus
whatever `deny_cidrs` subtracts) or `deny_all` (nothing outbound except
`allow_cidrs`).

How it is enforced (one dedicated iptables chain per sandbox, deny rules
before allow rules so deny wins on overlap, uplink-scoped rules so gateway
DNS is structurally exempt, the policy tied to the lease and re-applied
idempotently on resume and fork): see
[the egress internals page](../../docs-site/internals/egress-iptables.mdx).
Not on that page:
- A re-apply failure on resume or fork is a loud warning, not fatal: both are
  hard to redo, and this is a best-effort hardening layer, the same framing as
  the jailer.
- A snapshot-and-stop leaves the chain dormant but intact, like the tap; it
  is removed only when the lease is released (destroy, or deleting a held
  snapshot).

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
`--allow-cidr`/`--deny-cidr` (CLI), live-verified end to end in all three with a real LAN address (pinged
with no policy, blocked under `deny_all`, reachable again with a matching
`allow_cidrs`). It was daemon-HTTP-only first, the same deliberate
first-slice cut as pool `max_count`. Reference project:
`examples/egress-policy`.

## Hot-path fix

The `iptables-restore --noflush` batching is explained on the
[egress internals page](../../docs-site/internals/egress-iptables.mdx)
(8.3ms to 3.1ms, -63%, a new `egress_apply` phase metric). Not on that page:
measured over 17 fresh creates as the
`create_phase_duration_ms{phase="egress_apply"}` delta with a 6-CIDR policy
(9 spawns before, about 3 after: one `iptables-restore` plus the two shared
`FORWARD` calls). It was flagged as a real, unmeasured cost by the same audit
that found the `Vm::call` retry bug (see [findings](../findings.md),
finding 11). Unit tests and clippy clean; full integration suite 300/300 at
the time, including `19-egress.sh`.

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

# Tags and sandbox metadata

Key/value tags on sandboxes (environment, team, owner) for filtering and
listing.

## Durable sandbox history (sqlite)

New `sandkiln-store` crate (`rusqlite`, bundled), `GET /sandboxes/history`
(`?live_only=`, `?limit=`), `AppState::history: HistoryStore`.

Scope, stated honestly: this is not live sandboxes surviving a restart. A
live `Sandbox` owns a real Firecracker `Child` with open API and vsock
sockets, and nothing (the jailer included) can re-adopt one after the
daemon that spawned it is gone. A daemon restart of any kind orphans every
live process; `scripts/sandkilnd-ctl.sh` does a best-effort orphan `pkill`
for that reason. (Graceful SIGTERM now snapshots live sandboxes first, see
finding 16 in [findings](../findings.md).)

What sqlite buys is a durable record of every sandbox that ever existed: id,
name, tags, image, created-at and how it ended (`destroyed`, `snapshotted`
with the snapshot id, or `orphaned_by_restart`). At startup any record still
marked live becomes `orphaned_by_restart`, unconditionally, since nothing
can make it genuinely live. Verified live including a real restart; 15
integration checks cover the create/destroy/snapshot paths (the restart case
is manual).

Write latency: sqlite's defaults fsync twice per write, so the store uses
`journal_mode=WAL` and `synchronous=NORMAL` (1897.8us to 37.1us, see
[benchmarks](../benchmarks.md)).

## Guest-accessible metadata service

Firecracker's native MMDS, not sandkiln-built. A sandbox with a name or tags
serves `{id, name, tags}` at `http://169.254.169.254/` inside the guest,
answered by Firecracker's device model at the network layer. No vsock, guest
agent, protocol, daemon route or SDK/CLI change: it is automatic from the
`tags`/`name` already given at create.

- Configured V2 (token-gated: the guest must `PUT
  http://169.254.169.254/latest/api/token` before any `GET`), because a
  sandbox may run untrusted code that could SSRF an unauthenticated
  endpoint.
- Gotcha: `GET /` without an `Accept` header returns an AWS-IMDS-style
  newline list of key names (`id`, `name`, `tags`), not JSON. Send
  `Accept: application/json` for content.
- Verified by curling from inside a real sandbox (5 integration checks).
- Not wired: live updates (no mutable-tags API exists; `PATCH /mmds` would
  serve it, but needs the re-init workaround from
  [pools and lineage](pools-and-lineage.md)) and exposing resource config
  alongside `id`/`name`/`tags`.

# SDK and CLI design

## Zero-runtime-dependency principle

`packages/sdk` (JS/TS, [`sandkiln` on npm](https://www.npmjs.com/package/sandkiln))
has **no runtime dependencies at all** — `http.ts` wraps the platform's native
`fetch`, nothing else. The Python package's *sync* client (`_http.py`) is a
small wrapper over the standard library's `http.client`. The reasoning is the
same one `sandkiln-vmm` already applies on the Rust side for talking to
Firecracker's own API: a client library that talks JSON over HTTP doesn't need
a full framework's worth of dependencies to do it, and fewer dependencies means
a smaller, more auditable supply chain for something that will run
credentials/tokens through it.

## The async Python client — a hand-rolled HTTP/1.1 client, not a new dependency

`AsyncSandbox`/`AsyncDrive`/`AsyncImage`/`AsyncPool` are **fully separate
classes** from the sync `Sandbox`/`Drive`/`Image`/`Pool` — not one class with
both sync and async methods. Reasoning stated plainly: a sync call in an async
context blocks the event loop *silently* — worse than a wrong-import error,
since nothing fails loudly, it just quietly stalls concurrency.

- Built on `_http_async.py`: `asyncio.open_connection` directly, one connection
  per request (`Connection: close`, no keep-alive), a hand-rolled status-line +
  header parser, `Content-Length`-bounded body read via `reader.readexactly` —
  to preserve the same zero-extra-runtime-dependency principle as the sync
  client, rather than pulling in `aiohttp`/`httpx`.
- All dataclasses and body-building helpers (`DriveAttachment`, `ExecResult`,
  `_build_rate_limit`, `_build_egress`, ...) are **shared by import** from the
  sync modules — pure data/formatting functions with no I/O, nothing to
  duplicate between sync and async.
- Live-verified including genuine concurrency via `asyncio.gather` against a
  real daemon, not just typechecked.

## The CLI wraps the JS/TS SDK, not the HTTP API directly

`packages/cli` (`kiln`) depends on the published `sandkiln` npm package and
`commander` for argument parsing — every CLI command is a thin layer over an
SDK call, so the CLI can never drift from what the SDK actually does (no
separate HTTP-calling code path to keep in sync).

## What every layer stays in sync on

Adding a feature in this project always means checking (and usually touching)
every layer in this order:
`protocol → guest-agent → vmm → daemon (routes + structs + persistence) → JS SDK
→ Python SDK (sync, then async) → CLI → examples → docs`. This isn't a
suggestion — a new daemon endpoint with no SDK method and no CLI command is
treated as "started," not "done" (see the env-vars, mounts, and egress
features for the precedent: each landed across every layer, not just the
daemon route).

## Status

Both SDKs and the CLI: done, published, live-verified end to end against a real
daemon (not just typechecked in isolation).

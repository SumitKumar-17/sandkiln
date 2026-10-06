# AGENTS.md — sandkiln (Python SDK)

Read root `AGENTS.md` first.

## What this package is

Mirrors `packages/sdk` (JS/TS) exactly — same operations, same daemon,
Python-idiomatic naming (`run_command`, snake_case). Check the JS SDK
first before adding anything; the two should never drift in capability.

Ships two parallel APIs: sync (`Sandbox`/`Drive`/`Image`/`Pool`, on
`urllib`) and `asyncio`-native (`AsyncSandbox`/`AsyncDrive`/`AsyncImage`/
`AsyncPool`, on `asyncio.open_connection` — `_http_async.py`).
**Deliberately separate class hierarchies, not sync+async methods on one
class**: a sync call in an async context blocks the event loop silently
(no exception) — worse than an `AttributeError` from the wrong import.
Async classes re-export every dataclass/body-building helper from the
sync modules (`DriveAttachment`, `_build_rate_limit`, `_build_egress`) —
pure data/formatting, nothing "sync" to mirror.

Zero runtime dependencies (`urllib`/`asyncio` stdlib only, matching the
JS SDK's native-`fetch` choice). `_http_async.py` hand-rolls the same
~90-line minimal HTTP/1.1 shape `sandkiln-vmm` hand-rolls in Rust for
Firecracker's API — one connection per request, no keep-alive (this
SDK's calls are occasional, not high-throughput).

## Files

- **`sandbox.py`** — `Sandbox`, `ExecResult`/`SandboxInfo`. Mirrors
  `sandbox.ts` structurally. `preview_url` is pure/network-free, appends
  the token as `?token=`; unlike the JS SDK, the sandbox id isn't
  URL-quoted (matches this file's existing convention). `create`'s
  `image_id` boots from a registered image.
- **`image.py` / `drive.py` / `pool.py`** — classmethod-only, no instance
  state, mirroring `image.ts`/`drive.ts`/`pool.ts`. Drive attachment goes
  through `Sandbox.create`'s `drives` arg (`DriveAttachment`, defined in
  `sandbox.py`). `pool.py` takes a caller-given id like `image.py`'s
  `register`; no "claim" method — claiming is transparent via
  `Sandbox.create()` matching a configured pool.
- **`asandbox.py`/`adrive.py`/`aimage.py`/`apool.py`** — async mirrors,
  same names/args/returns, `async def` + awaited. Change a sync method,
  change its async mirror the same way.
- **`_http.py` / `_http_async.py`** — the one `urllib.request` /
  `asyncio.open_connection` call site each; same `SandkilnApiError`
  shape, different transport.
- **`_config.py`** — env fallback (`SANDKILN_DAEMON_URL`/
  `SANDKILN_AUTH_TOKEN`), matching `config.ts` exactly.
- **`errors.py`** — `SandkilnApiError(status, message)`.
- **`py.typed`** — empty PEP 561 marker; ships in the wheel automatically
  via hatchling's package config, no extra `pyproject.toml` entry needed.

## The bug that already happened here

**Never name a method the same as a builtin used in a type hint
elsewhere in the same class.** `Sandbox.list()` shadowed `list` for
`run_command()`'s `list[str]` hint — crashed at import time on Python
<3.14 (3.14 defers annotation evaluation per PEP 649 and didn't
reproduce it; CI on 3.12 did). Fixed with `from __future__ import
annotations` at the top of every file using `X | None`/`list[X]`/etc. —
already present everywhere; keep it in new files, and don't trust local
3.14 testing to catch this class of bug. Also required for real 3.9
compat (this package declares `requires-python = ">=3.9"`).

## Testing

`tests/test_preview_url.py` (stdlib `unittest`, mirrors the JS SDK's
`previewUrl.test.js`) covers the one pure logic piece. Run directly
(`python packages/python/tests/test_preview_url.py`, not `unittest
discover` — no `tests/__init__.py`). Everything else needs a live daemon.

## Building and verifying

```
python3 -m venv /tmp/some-venv
/tmp/some-venv/bin/pip install -e .
/tmp/some-venv/bin/python3 -c "import sandkiln; sandkiln.Sandbox"
```
That import check is the minimum bar (what caught the bug above, what CI
runs) — not sufficient alone. Verify against a real `sandkilnd` the same
way as `packages/sdk/AGENTS.md` (swap the Node script for Python).
`python -m build` (needs `pip install build`) for the actual sdist/wheel
when touching `pyproject.toml`.

## Non-obvious things

- `Sandbox.attach(id, ...)` does zero network calls, same reasoning as
  the JS SDK's `attach`.

## Publishing

**Published**: [`sandkiln` on PyPI](https://pypi.org/project/sandkiln/).
`.github/workflows/publish-python-sdk.yml` triggers on manual dispatch or
a `py-v*.*.*` tag, builds sdist/wheel, and on a tag push checks the tag
against `pyproject.toml`'s version before publishing via PyPI OIDC
trusted publishing (no stored token). To ship: bump `project.version`,
then push a matching tag or run the workflow manually (manual dispatch
skips the version-mismatch guard — double-check the bump landed first).

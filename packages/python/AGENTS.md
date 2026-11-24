# AGENTS.md: sandkiln (Python SDK)

Read root `AGENTS.md` first. [`sandkiln` on PyPI](https://pypi.org/project/sandkiln/). It mirrors `packages/sdk`
(same operations, snake_case naming); check the JS SDK first and never let capability drift. Standard library only.

It ships two parallel APIs: sync (`Sandbox`, `Drive`, `Image`, `Pool` on `urllib`) and asyncio (`AsyncSandbox`,
`AsyncDrive`, `AsyncImage`, `AsyncPool` on `asyncio.open_connection`). They are deliberately separate classes: a sync call
in an async context blocks the event loop silently. Async classes re-export the pure dataclasses and body builders
(`DriveAttachment`, `_build_rate_limit`, `_build_egress`) from the sync modules.

## Files (`src/sandkiln/`)

- **`sandbox.py`**: `Sandbox`, `ExecResult`, `SandboxInfo`, structurally mirroring `sandbox.ts`. `preview_url` is
  pure and appends `?token=`.
- **`image.py`, `drive.py`, `pool.py`, `status.py`**: classmethod-only, mirroring the TS files. Drives attach through
  `Sandbox.create(drives=...)`. No pool claim method.
- **`asandbox.py`, `adrive.py`, `aimage.py`, `apool.py`, `astatus.py`**: async mirrors, same names and arguments. Change a sync
  method, change its mirror the same way.
- **`_http.py`, `_http_async.py`**: the one `urllib` and `asyncio` call site; same `SandkilnApiError` (`errors.py`).
  The async client is a minimal HTTP/1.1 implementation, one connection per request.
- **`tunnel.py`, `atunnel.py`**: the only WebSocket feature, so both hand-roll RFC 6455 (handshake verified against the
  spec's test vector, masked frames, no fragmentation, which is fine for this protocol's small payloads) and the tunnel's
  binary frame (shared with `tunnel.ts` and the daemon: change one, change all three). `atunnel.py` imports the pure
  helpers from `tunnel.py`; only I/O is written twice.
- **`_config.py`**: env fallback, matching `config.ts`. **`py.typed`**: PEP 561 marker.

## Invariants

- Never name a method after a builtin used in a type hint in the same class. `Sandbox.list()` shadowed `list` in
  `list[str]` and crashed at import on Python below 3.14 (3.14 defers annotations, so local runs hid it). Keep
  `from __future__ import annotations` in every file; the package declares `>=3.9`.
- `Sandbox.attach(id)` makes zero network calls.

## Testing and release

`python packages/python/tests/test_preview_url.py` and `test_status.py` (stdlib `unittest`, run directly; there is no
`tests/__init__.py`). Minimum bar: `pip install -e .` in a venv, then `python -c "import sandkiln; sandkiln.Sandbox"`
(what CI runs). Also verify against a real `sandkilnd` as in `packages/sdk/AGENTS.md`, and `python -m build` when
touching `pyproject.toml`.

Publishing: `.github/workflows/publish-python-sdk.yml` runs on manual dispatch or a `py-v*.*.*` tag, checks the tag
against `pyproject.toml` on tag pushes, and uses PyPI OIDC trusted publishing (no stored token). Bump
`project.version` first; manual dispatch skips the version guard.

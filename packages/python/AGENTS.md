# AGENTS.md — sandkiln (Python SDK)

Read the root `AGENTS.md` first for project-wide conventions. This file
is scoped to this one package.

## What this package is

The Python client, mirroring `packages/sdk` (the JS/TS SDK) exactly —
same operations, same daemon, Python-idiomatic naming (`run_command` not
`runCommand`, snake_case fields). If you're adding something here, check
whether the JS SDK has it first; these two should never drift apart in
capability, only in the idioms of each language.

Ships two parallel APIs: a synchronous one (`Sandbox`/`Drive`/`Image`/
`Pool`, built on `urllib`) and an `asyncio`-native one (`AsyncSandbox`/
`AsyncDrive`/`AsyncImage`/`AsyncPool`, built on `asyncio.open_connection`
— see `_http_async.py`). **Deliberately two separate class hierarchies,
not one class with both sync and async methods on it**: they share no
state, and mixing them on one class invites calling the wrong one by
accident — a sync call in an async context blocks the event loop
silently (no exception, just a stall), which is a much worse failure
mode than a clear `AttributeError` from having imported the wrong class
name. The async classes share every dataclass and body-building helper
with their sync counterparts (`sandbox.py`'s `DriveAttachment`,
`_build_rate_limit`, `_build_egress`, etc.) — those are pure data/
formatting with no I/O, so there's nothing "sync" about them to mirror;
only re-export them from `asandbox.py`/`adrive.py`/etc. rather than
redefining anything.

Zero runtime dependencies on purpose — `urllib`/`asyncio` from the
standard library, not `requests`/`aiohttp`/`httpx`. Match this if you add
anything; don't introduce a dependency the JS SDK's equivalent doesn't
need either (it uses native `fetch`). The async HTTP client
(`_http_async.py`) hand-rolls the same ~90-line minimal HTTP/1.1 shape
`sandkiln-vmm` already hand-rolls in Rust for Firecracker's own API (see
`core/crates/vmm/src/firecracker_api.rs`) — one connection per request,
no keep-alive, since this
SDK's call pattern is occasional request/response calls against a local
or nearby daemon, not a high-throughput client where reuse would matter.

## Files

- `sandbox.py` — the `Sandbox` class, `ExecResult`/`SandboxInfo`
  dataclasses. Structurally mirrors `packages/sdk/src/sandbox.ts` — if
  you change one, change the other the same way. `preview_url` is pure and
  network-free like `attach` — it just builds the URL for the daemon's
  `/sandboxes/:id/preview/:port` reverse proxy (see
  `core/crates/daemon/src/routes_preview.rs`), appending the auth token as
  a `?token=` query parameter rather than a header when one is configured,
  since the caller is typically a browser tab or `<iframe>`. Unlike the JS
  SDK's `previewUrl`, the sandbox id isn't URL-quoted here — matching this
  file's own existing convention (`run_command`/`read_file`/etc. don't
  quote it either), not an oversight. `Sandbox.create`'s `image_id`
  boots from a registered image (see `image.py`) instead of the daemon's
  configured default rootfs.
- `image.py` — the `Image` class, `ImageInfo` dataclass. Mirrors
  `packages/sdk/src/image.ts`: `register`/`list`/`delete` are all
  classmethods, no instance state — an image has no behavior besides
  delete, and delete only ever needs an id.
- `drive.py` — the `Drive` class, `DriveInfo`/`DriveHolder` dataclasses.
  Mirrors `packages/sdk/src/drive.ts` the same way `image.py` mirrors
  `image.ts` — same static-classmethod shape, no instance state. Attach
  a drive at create time via `Sandbox.create`'s `drives` argument
  (`DriveAttachment` dataclass, defined in `sandbox.py` since it's
  create-time input, not a `Drive`-class operation).
- `pool.py` — the `Pool` class, `PoolInfo` dataclass. Mirrors
  `packages/sdk/src/pool.ts` — same static-classmethod shape, caller-given
  `id` like `image.py`'s `register` (not a server-generated one like
  `drive.py`'s `create`). No "claim from pool" method here at all —
  claiming is entirely transparent, done by a plain `Sandbox.create()`
  whose image/resources match a configured pool.
- `asandbox.py`/`adrive.py`/`aimage.py`/`apool.py` — `AsyncSandbox`/
  `AsyncDrive`/`AsyncImage`/`AsyncPool`: `asyncio`-native mirrors of
  `Sandbox`/`Drive`/`Image`/`Pool`, same method names/arguments/return
  shapes, just `async def` and awaited. See this file's own module doc
  comments for why these are separate classes rather than sync+async
  methods on one class. Changing a method on the sync side means
  changing its async mirror the same way — the same lockstep discipline
  `sandbox.py`/`sandbox.ts` already require across languages, just
  within this one package instead.
- `_http.py` — the one place `urllib.request` gets called. Leading
  underscore: not part of the public API, same convention as `_config.py`.
- `_http_async.py` — the one place `asyncio.open_connection` gets
  called, for `AsyncSandbox`/etc. Same `SandkilnApiError` shape and error
  handling as `_http.py`, different transport underneath.
- `_config.py` — env var fallback resolution
  (`SANDKILN_DAEMON_URL`/`SANDKILN_AUTH_TOKEN`), matching the JS SDK's
  `config.ts` exactly (same env var names, same default).
- `errors.py` — `SandkilnApiError(status, message)`, matching the JS
  SDK's `SandkilnApiError` shape.
- `py.typed` — empty PEP 561 marker so downstream type checkers (mypy,
  pyright) trust this package's type hints instead of treating it as
  untyped. Ships in the wheel automatically — `python -m build` +
  `unzip -l dist/*.whl` confirmed it, no `pyproject.toml` change needed;
  hatchling's `[tool.hatch.build.targets.wheel]` `packages =
  ["src/sandkiln"]` already includes every file under that directory.

## The bug that already happened here — read before adding a method

**Never name a method the same as a builtin type you use in a type hint
elsewhere in the same class.** `Sandbox.list()` shadowed the builtin
`list` for the `list[str]` annotation on `run_command()` (defined later
in the class body) — this crashed at *import time* with `TypeError:
'classmethod' object is not subscriptable', but only on Python <3.14;
this session's local Python 3.14 defers annotation evaluation by default
(PEP 649) and didn't reproduce it, CI on 3.12 did. Fixed with `from
__future__ import annotations` at the top of every file using modern
union/generic syntax (`X | None`, `list[X]`, `dict[K, V]`) — this is
already present in every file here; **keep it** if you add a new file
with this kind of type hint, and remember that Python 3.14 will not
catch this class of bug locally even if you test there. This also
happens to be required for real 3.9 compatibility (`X | None` syntax is
a 3.10+ runtime feature without deferred annotations) — this package
declares `requires-python = ">=3.9"`.

## Testing

`tests/test_preview_url.py` covers `Sandbox.preview_url`'s pure
URL-building logic with stdlib `unittest` (mirrors
`packages/sdk/test/previewUrl.test.js` on the JS side, zero added
dependency — same reasoning as that package's `node:test` choice). Run:
`python packages/python/tests/test_preview_url.py` (a plain
`unittest.main()` invocation, not `unittest discover`, since there's no
`tests/__init__.py` to make the directory importable as a package for
discovery). Every other method needs a live daemon to test meaningfully,
which is why this is the only test file here today.

## Building and verifying

```
python3 -m venv /tmp/some-venv
/tmp/some-venv/bin/pip install -e .
/tmp/some-venv/bin/python3 -c "import sandkiln; sandkiln.Sandbox"
```
That import check is the minimum bar — it's exactly what caught the bug
above, and exactly what CI runs. Beyond that, **verify against a real
`sandkilnd`** the same way described in `packages/sdk/AGENTS.md` — swap
the Node script for a small Python one.

`python -m build` (needs `pip install build`) produces the actual sdist/
wheel CI and the publish workflow use — run it if you're touching
`pyproject.toml` specifically.

## Non-obvious things specific to this package

- `Sandbox.attach(id, ...)` exists for the same reason as the JS SDK's:
  a fresh process reconstructing a handle to an existing sandbox without
  a network round-trip.

## Publishing

**Published**: [`sandkiln` on PyPI](https://pypi.org/project/sandkiln/),
currently at `0.1.0`. Code-side, this package is ready: `pyproject.toml`
is filled in with real values (no placeholders), the package builds a
clean wheel/sdist (`python -m build`), and it carries a `py.typed`
marker.

Publishing is automated — `.github/workflows/publish-python-sdk.yml`:
triggers on a manual dispatch or a `py-v*.*.*` tag push, builds the
sdist/wheel, and on a tag push additionally checks the tag's version
against `pyproject.toml`'s `project.version` and fails the run if they
don't match. It then publishes via `pypa/gh-action-pypi-publish` using
PyPI's OIDC trusted publishing — no stored token, no 2FA-on-publish
friction (the workflow's own `id-token: write` permission is what makes
the OIDC exchange possible). The one-time trusted-publisher registration
on pypi.org (owner `SumitKumar-17`, repo `sandkiln`, workflow file
`publish-python-sdk.yml`) is done.

**To ship a new version**: bump `project.version` in `pyproject.toml`,
then either push a matching `py-vX.Y.Z` tag or run the workflow manually
(`gh workflow run publish-python-sdk.yml`). A tag push additionally
guards against a version mismatch; a manual dispatch does not, so
double-check the version bump landed first.

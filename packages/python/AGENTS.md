# AGENTS.md — sandkiln (Python SDK)

Read the root `AGENTS.md` first for project-wide conventions. This file
is scoped to this one package.

## What this package is

The Python client, mirroring `packages/sdk` (the JS/TS SDK) exactly —
same operations, same daemon, Python-idiomatic naming (`run_command` not
`runCommand`, snake_case fields, a plain synchronous API — no `asyncio`).
If you're adding something here, check whether the JS SDK has it first;
these two should never drift apart in capability, only in the idioms of
each language.

Zero runtime dependencies on purpose — `urllib` from the standard
library, not `requests`. Match this if you add anything; don't introduce
a dependency the JS SDK's equivalent doesn't need either (it uses native
`fetch`).

## Files

- `sandbox.py` — the `Sandbox` class, `ExecResult`/`SandboxInfo`
  dataclasses. Structurally mirrors `packages/sdk/src/sandbox.ts` — if
  you change one, change the other the same way.
- `_http.py` — the one place `urllib.request` gets called. Leading
  underscore: not part of the public API, same convention as `_config.py`.
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

Not yet published to PyPI. Code-side, this package is ready:
`pyproject.toml` is filled in with real values (no placeholders), the
package builds a clean wheel/sdist (`python -m build`), and it carries a
`py.typed` marker.

**Already automated** — `.github/workflows/publish-python-sdk.yml`:
triggers on a manual dispatch or a `py-v*.*.*` tag push, builds the
sdist/wheel, and on a tag push additionally checks the tag's version
against `pyproject.toml`'s `project.version` and fails the run if they
don't match. It then publishes via `pypa/gh-action-pypi-publish` using
PyPI's OIDC trusted publishing — no stored token, no 2FA-on-publish
friction (the workflow's own `id-token: write` permission is what makes
the OIDC exchange possible).

**Still manual, one-time, needs the account owner** — trusted publishing
has to be registered on pypi.org *before* the workflow above can
succeed: register the `sandkiln` project name on PyPI, then under its
"Publishing" settings add a trusted publisher pointing at owner
`SumitKumar-17`, repo `sandkiln`, workflow file
`publish-python-sdk.yml` (environment left blank unless one is added to
the workflow later). This needs the pypi.org account itself — no agent
or CI job can do it. Once registered, either push a `py-v0.1.0` tag or
run the workflow manually to publish.

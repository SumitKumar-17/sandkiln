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
- **`_ws.py` / `_ws_async.py`** — the one hand-rolled RFC 6455 client
  (the standard library has none), shared by the tunnel, `pty()` and
  `attach_logs()`. `_ws.py` holds the pure parts (handshake request and
  `Sec-WebSocket-Accept` check, `encode_frame`/`apply_mask`, `MessageAssembler`
  for fragmented messages and interleaved control frames, the `?token=` path
  helper, mapping a pre-upgrade HTTP error to `SandkilnApiError`) plus the
  blocking `WebSocketConnection`; `_ws_async.py` is the `asyncio` I/O
  (`AsyncWebSocketConnection` reads in a task into a bounded queue, so a
  `recv(timeout=)` cancel cannot lose half a frame). Pure helpers are
  imported, never duplicated. Peer-declared lengths are untrusted
  (`MAX_MESSAGE_BYTES`). Auth is `?token=`, as the daemon's WebSocket
  routes accept it (`require_preview_token`).
- **`streams.py` / `astreams.py`** — `PtySession`/`LogStream` (and async
  mirrors), `ExecStreamSession`, `LogOutput`. Thin session objects over a
  connection (`send`/`recv(timeout)`/iterate/context manager) instead of the
  raw socket the JS SDK returns. Ids are URL-quoted into the request line
  (`pty_path`, `logs_path`) so they cannot inject handshake lines. The
  daemon applies `cols`/`rows` once; there is no live resize.
- **`transfer.py` / `atransfer.py`** — `upload_dir`/`download_dir` and
  `TransferResult`, mirroring `packages/sdk/src/transfer.ts`. `download_dir`
  handles untrusted guest input: `safe_child` (one plain path component),
  `symlink_stays_inside` (resolves links already on disk, stricter than the JS
  lexical check), duplicate-name refusal, `O_NOFOLLOW` writes, setuid bits
  dropped. Keep the two directions' rules in step with `transfer.ts`.
- **`tunnel.py` / `atunnel.py`** — `Sandbox.tunnel`/`AsyncSandbox.tunnel`:
  the tunnel's own small binary multiplexing frame (shared wire format
  with the JS SDK's `tunnel.ts` and the daemon's `http/routes/tunnel.rs` —
  change one, change all three) over `_ws.py`. `tunnel.py` runs its relay
  loop on a background `threading.Thread`; `atunnel.py` mirrors it as an
  `asyncio.Task`.
- **`_config.py`** — env fallback (`SANDKILN_DAEMON_URL`/
  `SANDKILN_AUTH_TOKEN`), matching `config.ts` exactly.
- **`errors.py`** — `SandkilnApiError(status, message, body)` and `SandkilnExecTimeoutError`, its 504 subclass raised by `run_command(timeout=)` when the guest killed the command (`stdout`, `stderr`, `timeout_secs`); `as_exec_timeout` is the pure conversion.
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

Run each file directly (`python packages/python/tests/test_websocket.py`,
not `unittest discover` — no `tests/__init__.py`); no test framework needed.
`test_preview_url.py` (stdlib `unittest`), `test_websocket.py` (RFC 6455
vectors, masking, reassembly, handshake, sync/async round trips against the
in-process server in `_fakews.py`), `test_streams.py` (`pty`/`attach_logs`/
`exec_stream` against fakes), `test_transfer.py` (path-safety helpers and
trees against an in-memory sandbox, including a hostile listing) are
plain `test_*` functions with a small runner, also valid under pytest.
`test_exec_timeout.py` and `test_status.py` import pytest. Anything touching a
real microVM (a PTY round trip, a streamed exec, a directory copy) needs a
live daemon: run a script against it as in the next section.

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
- `Sandbox.pty()`/`attach_logs()` connect immediately (a refused session is a
  real `SandkilnApiError` from before the upgrade), unlike `attach`.

## Publishing

**Published**: [`sandkiln` on PyPI](https://pypi.org/project/sandkiln/).
`.github/workflows/publish-python-sdk.yml` triggers on manual dispatch or
a `py-v*.*.*` tag, builds sdist/wheel, and on a tag push checks the tag
against `pyproject.toml`'s version before publishing via PyPI OIDC
trusted publishing (no stored token). To ship: bump `project.version`,
then push a matching tag or run the workflow manually (manual dispatch
skips the version-mismatch guard — double-check the bump landed first).

"""Async counterpart to `_http.request` -- same wire behavior, same
`SandkilnApiError` shape, but built on `asyncio.open_connection` instead
of `urllib.request` (which has no async form in the standard library).

Zero added dependencies, matching this SDK's existing zero-runtime-
dependency design (see `packages/python/AGENTS.md`) and the same
reasoning `sandkiln-vmm` already applies to Firecracker's own API: the
daemon speaks one small, fixed HTTP surface (JSON in, JSON out, no
chunked transfer, no redirects, no connection reuse needed for a
request pattern this infrequent) -- hand-rolling the ~60 lines a minimal
HTTP/1.1 client actually needs here is less code and less risk than
adding `aiohttp`/`httpx` for a shape neither would meaningfully simplify.

One connection per request, closed afterward. No keep-alive: this SDK's
own call pattern is occasional request/response calls against a local
or nearby daemon, not a high-throughput client where connection reuse
would matter.
"""

from __future__ import annotations

import asyncio
import json
import ssl as ssl_module
from typing import Any
from urllib.parse import urlsplit

from .errors import SandkilnApiError


async def request(
    base_url: str,
    method: str,
    path: str,
    auth_token: str | None = None,
    body: Any = None,
) -> Any:
    parsed = urlsplit(base_url)
    is_https = parsed.scheme == "https"
    host = parsed.hostname
    if host is None:
        raise SandkilnApiError(0, f"could not parse a host out of base_url: {base_url}")
    port = parsed.port or (443 if is_https else 80)
    full_path = f"{parsed.path.rstrip('/')}{path}" or "/"

    data: bytes | None = None
    headers = {"Host": host, "Connection": "close"}
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
        headers["Content-Length"] = str(len(data))
    if auth_token is not None:
        headers["Authorization"] = f"Bearer {auth_token}"

    try:
        ssl_context = ssl_module.create_default_context() if is_https else None
        reader, writer = await asyncio.open_connection(host, port, ssl=ssl_context)
    except OSError as error:
        raise SandkilnApiError(0, f"could not reach {base_url}: {error}") from None

    try:
        request_lines = [f"{method} {full_path} HTTP/1.1"]
        request_lines += [f"{key}: {value}" for key, value in headers.items()]
        writer.write(("\r\n".join(request_lines) + "\r\n\r\n").encode("ascii"))
        if data is not None:
            writer.write(data)
        await writer.drain()

        status, response_headers, raw_body = await _read_response(reader)
    except OSError as error:
        raise SandkilnApiError(0, f"could not reach {base_url}: {error}") from None
    finally:
        writer.close()
        try:
            await writer.wait_closed()
        except OSError:
            pass

    if status >= 400:
        raise SandkilnApiError(status, _extract_error_message(raw_body, status))
    if not raw_body:
        return None
    return json.loads(raw_body)


async def _read_response(reader: asyncio.StreamReader) -> tuple[int, dict[str, str], bytes]:
    status_line = await reader.readline()
    # "HTTP/1.1 200 OK\r\n" -> 200. The reason phrase is informational
    # only and not parsed -- nothing here reads it.
    status = int(status_line.split(b" ", 2)[1])

    headers: dict[str, str] = {}
    while True:
        line = await reader.readline()
        if line in (b"\r\n", b""):
            break
        name, _, value = line.decode("iso-8859-1").partition(":")
        headers[name.strip().lower()] = value.strip()

    content_length = int(headers.get("content-length", "0"))
    body = await reader.readexactly(content_length) if content_length > 0 else b""
    return status, headers, body


def _extract_error_message(raw_body: bytes, status: int) -> str:
    if raw_body:
        try:
            parsed = json.loads(raw_body)
            if isinstance(parsed, dict) and isinstance(parsed.get("error"), str):
                return parsed["error"]
        except (json.JSONDecodeError, UnicodeDecodeError):
            pass
    return f"request failed with status {status}"

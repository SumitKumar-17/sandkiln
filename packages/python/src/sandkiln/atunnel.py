"""Async counterpart to `tunnel.py` -- same wire behavior (handshake,
frame format, tunnel multiplexing), built on `asyncio.open_connection`
instead of blocking sockets + threads, matching `_http_async.py`'s own
reasoning for why this is a hand-rolled client rather than a new
dependency. Reuses every pure encode/decode helper from `tunnel.py`
(nothing "sync" about computing a handshake key or framing a payload) --
only the actual I/O is written twice, same convention the rest of this
package's async modules follow.
"""

from __future__ import annotations

import asyncio
import ssl as ssl_module
from urllib.parse import urlencode, urlsplit

from ._http_async import request as http_request_async
from .tunnel import (
    _OP_CLOSE,
    _OP_DATA,
    _OP_OPEN,
    _OPCODE_BINARY,
    _OPCODE_CLOSE,
    _OPCODE_PING,
    _OPCODE_PONG,
    _decode_tunnel_frame,
    _encode_tunnel_frame,
    _encode_ws_frame,
    _expected_accept,
    _generate_key,
)


async def _connect_websocket(
    base_url: str, path: str, auth_token: str | None
) -> tuple[asyncio.StreamReader, asyncio.StreamWriter]:
    parsed = urlsplit(base_url)
    is_tls = parsed.scheme in ("https", "wss")
    host = parsed.hostname
    if host is None:
        raise ValueError(f"base_url has no host: {base_url!r}")
    port = parsed.port or (443 if is_tls else 80)

    ssl_context = ssl_module.create_default_context() if is_tls else None
    reader, writer = await asyncio.open_connection(host, port, ssl=ssl_context)

    full_path = path
    if auth_token is not None:
        separator = "&" if "?" in path else "?"
        full_path = f"{path}{separator}{urlencode({'token': auth_token})}"

    key = _generate_key()
    request_lines = [
        f"GET {full_path} HTTP/1.1",
        f"Host: {host}:{port}",
        "Upgrade: websocket",
        "Connection: Upgrade",
        f"Sec-WebSocket-Key: {key}",
        "Sec-WebSocket-Version: 13",
        "",
        "",
    ]
    writer.write("\r\n".join(request_lines).encode("ascii"))
    await writer.drain()

    status_line = (await reader.readline()).decode("ascii", errors="replace")
    if " 101 " not in status_line:
        raise ConnectionError(f"websocket handshake failed: {status_line}")

    accept_value = None
    while True:
        line = await reader.readline()
        if line in (b"\r\n", b""):
            break
        if b":" in line:
            name, _, value = line.partition(b":")
            if name.strip().lower() == b"sec-websocket-accept":
                accept_value = value.strip().decode("ascii")
    if accept_value != _expected_accept(key):
        raise ConnectionError("websocket handshake failed: Sec-WebSocket-Accept did not match the expected value")

    return reader, writer


async def _read_ws_frame(reader: asyncio.StreamReader) -> tuple[int, bytes]:
    first_two = await reader.readexactly(2)
    opcode = first_two[0] & 0x0F
    masked = (first_two[1] & 0x80) != 0
    length = first_two[1] & 0x7F
    if length == 126:
        length = int.from_bytes(await reader.readexactly(2), "big")
    elif length == 127:
        length = int.from_bytes(await reader.readexactly(8), "big")
    mask_key = await reader.readexactly(4) if masked else None
    payload = await reader.readexactly(length)
    if mask_key is not None:
        payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    return opcode, payload


class AsyncTunnelHandle:
    """Async counterpart to `tunnel.TunnelHandle`. Call `await close()`
    when done forwarding -- safe to call more than once."""

    def __init__(
        self,
        tunnel_id: str,
        writer: asyncio.StreamWriter,
        relay_task: asyncio.Task,
        base_url: str,
        sandbox_id: str,
        auth_token: str | None,
    ) -> None:
        self.tunnel_id = tunnel_id
        self._writer = writer
        self._relay_task = relay_task
        self._base_url = base_url
        self._sandbox_id = sandbox_id
        self._auth_token = auth_token
        self._closed = False

    async def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        self._relay_task.cancel()
        self._writer.close()
        try:
            await self._writer.wait_closed()
        except OSError:
            pass
        try:
            await http_request_async(
                self._base_url, "DELETE", f"/sandboxes/{self._sandbox_id}/tunnel/{self.tunnel_id}", self._auth_token
            )
        except Exception:
            pass


async def open_tunnel(
    base_url: str,
    auth_token: str | None,
    sandbox_id: str,
    guest_port: int,
    local_port: int,
    local_host: str = "127.0.0.1",
) -> AsyncTunnelHandle:
    """Async counterpart to `tunnel.open_tunnel` -- see that function's
    doc comment for the full picture. The relay runs as a background
    `asyncio.Task` instead of a thread."""
    body = await http_request_async(base_url, "POST", f"/sandboxes/{sandbox_id}/tunnel", auth_token, {"guest_port": guest_port})
    tunnel_id = body["tunnel_id"]

    reader, writer = await _connect_websocket(base_url, f"/sandboxes/{sandbox_id}/tunnel/{tunnel_id}/ws", auth_token)

    send_lock = asyncio.Lock()
    local_connections: dict[str, tuple[asyncio.StreamReader, asyncio.StreamWriter]] = {}

    async def send_ws(opcode: int, payload: bytes) -> None:
        async with send_lock:
            writer.write(_encode_ws_frame(opcode, payload))
            await writer.drain()

    async def pump_local_to_ws(conn_id: str, local_reader: asyncio.StreamReader) -> None:
        try:
            while True:
                chunk = await local_reader.read(8192)
                if not chunk:
                    break
                await send_ws(_OPCODE_BINARY, _encode_tunnel_frame(_OP_DATA, conn_id, chunk))
        except (OSError, asyncio.IncompleteReadError):
            pass
        finally:
            local_connections.pop(conn_id, None)
            try:
                await send_ws(_OPCODE_BINARY, _encode_tunnel_frame(_OP_CLOSE, conn_id, b""))
            except OSError:
                pass

    async def relay_loop() -> None:
        try:
            while True:
                opcode, payload = await _read_ws_frame(reader)
                if opcode == _OPCODE_CLOSE:
                    break
                if opcode == _OPCODE_PING:
                    await send_ws(_OPCODE_PONG, payload)
                    continue
                if opcode != _OPCODE_BINARY:
                    continue
                decoded = _decode_tunnel_frame(payload)
                if decoded is None:
                    continue
                op, conn_id, data = decoded
                if op == _OP_OPEN:
                    try:
                        local_reader, local_writer = await asyncio.open_connection(local_host, local_port)
                    except OSError:
                        await send_ws(_OPCODE_BINARY, _encode_tunnel_frame(_OP_CLOSE, conn_id, b""))
                        continue
                    local_connections[conn_id] = (local_reader, local_writer)
                    asyncio.ensure_future(pump_local_to_ws(conn_id, local_reader))
                elif op == _OP_DATA:
                    conn = local_connections.get(conn_id)
                    if conn is not None:
                        try:
                            conn[1].write(data)
                            await conn[1].drain()
                        except OSError:
                            local_connections.pop(conn_id, None)
                elif op == _OP_CLOSE:
                    conn = local_connections.pop(conn_id, None)
                    if conn is not None:
                        conn[1].close()
        except (OSError, ConnectionError, asyncio.IncompleteReadError):
            pass
        finally:
            for _, local_writer in local_connections.values():
                local_writer.close()
            local_connections.clear()

    relay_task = asyncio.ensure_future(relay_loop())

    return AsyncTunnelHandle(tunnel_id, writer, relay_task, base_url, sandbox_id, auth_token)

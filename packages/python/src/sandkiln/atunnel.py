"""Async counterpart to `tunnel.py` -- same wire behavior (handshake,
frame format, tunnel multiplexing), built on `asyncio.open_connection`
instead of blocking sockets + threads, matching `_http_async.py`'s own
reasoning for why this is a hand-rolled client rather than a new
dependency. The WebSocket handshake and framing live in `_ws.py` /
`_ws_async.py`; the tunnel's own multiplexing frame helpers come from
`tunnel.py`. Only the actual I/O is written twice, same convention the
rest of this package's async modules follow.
"""

from __future__ import annotations

import asyncio

from ._http_async import request as http_request_async
from ._ws import OPCODE_BINARY, OPCODE_CLOSE, OPCODE_PING, OPCODE_PONG, MessageAssembler, encode_frame
from ._ws_async import connect_websocket, read_frame
from .tunnel import _OP_CLOSE, _OP_DATA, _OP_OPEN, _decode_tunnel_frame, _encode_tunnel_frame


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

    reader, writer = await connect_websocket(base_url, f"/sandboxes/{sandbox_id}/tunnel/{tunnel_id}/ws", auth_token)

    send_lock = asyncio.Lock()
    local_connections: dict[str, tuple[asyncio.StreamReader, asyncio.StreamWriter]] = {}

    async def send_ws(opcode: int, payload: bytes) -> None:
        async with send_lock:
            writer.write(encode_frame(opcode, payload))
            await writer.drain()

    async def pump_local_to_ws(conn_id: str, local_reader: asyncio.StreamReader) -> None:
        try:
            while True:
                chunk = await local_reader.read(8192)
                if not chunk:
                    break
                await send_ws(OPCODE_BINARY, _encode_tunnel_frame(_OP_DATA, conn_id, chunk))
        except (OSError, asyncio.IncompleteReadError):
            pass
        finally:
            local_connections.pop(conn_id, None)
            try:
                await send_ws(OPCODE_BINARY, _encode_tunnel_frame(_OP_CLOSE, conn_id, b""))
            except OSError:
                pass

    async def relay_loop() -> None:
        assembler = MessageAssembler()
        try:
            while True:
                fin, opcode, payload = await read_frame(reader)
                event = assembler.feed(fin, opcode, payload)
                if event is None:
                    continue
                opcode, payload = event
                if opcode == OPCODE_CLOSE:
                    break
                if opcode == OPCODE_PING:
                    await send_ws(OPCODE_PONG, payload)
                    continue
                if opcode != OPCODE_BINARY:
                    continue
                decoded = _decode_tunnel_frame(payload)
                if decoded is None:
                    continue
                op, conn_id, data = decoded
                if op == _OP_OPEN:
                    try:
                        local_reader, local_writer = await asyncio.open_connection(local_host, local_port)
                    except OSError:
                        await send_ws(OPCODE_BINARY, _encode_tunnel_frame(_OP_CLOSE, conn_id, b""))
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

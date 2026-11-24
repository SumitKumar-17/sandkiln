"""`asyncio` counterpart to `_ws.py`: the same handshake, framing and
message rules on `asyncio.open_connection`, with every pure helper imported
from there. Only the I/O is written twice, like every other async mirror in
this package.

`AsyncWebSocketConnection` reads in a background task that feeds a bounded
queue, so `recv(timeout=)` can be cancelled by the timeout without losing
bytes of a half-read frame, and a consumer that stops reading pushes back on
the socket instead of growing memory.
"""

from __future__ import annotations

import asyncio
import ssl as ssl_module
from urllib.parse import urlsplit

from ._ws import (
    MAX_MESSAGE_BYTES,
    OPCODE_CLOSE,
    OPCODE_PING,
    OPCODE_PONG,
    OPCODE_BINARY,
    OPCODE_TEXT,
    MessageAssembler,
    apply_mask,
    build_handshake_request,
    check_accept,
    decode_message,
    encode_close_payload,
    encode_frame,
    error_from_response,
    generate_key,
    parse_frame_head,
    parse_header_line,
    parse_status_code,
    ws_path,
)

_QUEUE_DEPTH = 256


async def connect_websocket(
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
    try:
        key = generate_key()
        writer.write(build_handshake_request(host, port, ws_path(path, auth_token), key))
        await writer.drain()

        status = parse_status_code((await reader.readline()).decode("ascii", errors="replace"))
        headers: dict[str, str] = {}
        while True:
            line = await reader.readline()
            if line in (b"\r\n", b""):
                break
            parsed_header = parse_header_line(line.rstrip(b"\r\n"))
            if parsed_header is not None:
                headers[parsed_header[0]] = parsed_header[1]
        if status != 101:
            length = min(int(headers.get("content-length", "0") or 0), 65536)
            raise error_from_response(status, await reader.readexactly(length) if length else b"")
        check_accept(key, headers.get("sec-websocket-accept"))
    except BaseException:
        writer.close()
        raise
    return reader, writer


async def read_frame(reader: asyncio.StreamReader) -> tuple[bool, int, bytes]:
    fin, opcode, masked, length = parse_frame_head(await reader.readexactly(2))
    if length == 126:
        length = int.from_bytes(await reader.readexactly(2), "big")
    elif length == 127:
        length = int.from_bytes(await reader.readexactly(8), "big")
    if length > MAX_MESSAGE_BYTES:
        raise ConnectionError("websocket frame exceeds the size limit")
    mask_key = await reader.readexactly(4) if masked else None
    payload = await reader.readexactly(length)
    if mask_key is not None:
        payload = apply_mask(payload, mask_key)
    return fin, opcode, payload


class AsyncWebSocketConnection:
    """A connected `asyncio` WebSocket: `await recv()` yields whole messages
    (`bytes` for binary, `str` for text) and answers pings itself."""

    def __init__(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        self._reader = reader
        self._writer = writer
        self._queue: asyncio.Queue[bytes | str | None] = asyncio.Queue(maxsize=_QUEUE_DEPTH)
        self._send_lock = asyncio.Lock()
        self._finished = False
        self.closed = False
        self._reader_task = asyncio.ensure_future(self._read_loop())

    @classmethod
    async def connect(cls, base_url: str, path: str, auth_token: str | None) -> AsyncWebSocketConnection:
        reader, writer = await connect_websocket(base_url, path, auth_token)
        return cls(reader, writer)

    async def send(self, data: bytes | str) -> None:
        if isinstance(data, str):
            await self._send_frame(OPCODE_TEXT, data.encode("utf-8"))
        else:
            await self._send_frame(OPCODE_BINARY, bytes(data))

    async def _send_frame(self, opcode: int, payload: bytes) -> None:
        async with self._send_lock:
            self._writer.write(encode_frame(opcode, payload))
            await self._writer.drain()

    async def _read_loop(self) -> None:
        assembler = MessageAssembler()
        try:
            while True:
                fin, opcode, payload = await read_frame(self._reader)
                event = assembler.feed(fin, opcode, payload)
                if event is None:
                    continue
                kind, data = event
                if kind == OPCODE_PING:
                    await self._send_frame(OPCODE_PONG, data)
                elif kind == OPCODE_CLOSE:
                    try:
                        await self._send_frame(OPCODE_CLOSE, data[:2])
                    except OSError:
                        pass
                    break
                elif kind != OPCODE_PONG:
                    await self._queue.put(decode_message(kind, data))
        except (OSError, ConnectionError, asyncio.IncompleteReadError):
            pass
        finally:
            self._finished = True
            self.closed = True
            self._writer.close()
            # The sentinel must not block behind a full queue nobody is
            # draining: if there is no room, a consumer that does drain will
            # see `_finished` once the queue empties.
            try:
                self._queue.put_nowait(None)
            except asyncio.QueueFull:
                pass

    async def recv(self, timeout: float | None = None) -> bytes | str | None:
        """The next message, or `None` once the peer closed. With a
        `timeout` (seconds), raises `TimeoutError` if nothing arrives in
        time; the connection stays usable."""
        if self._finished and self._queue.empty():
            return None
        try:
            item = await asyncio.wait_for(self._queue.get(), timeout)
        except asyncio.TimeoutError:
            raise TimeoutError("no websocket message within the timeout") from None
        if item is None:
            # Leave the sentinel for any later recv().
            self._queue.put_nowait(None)
        return item

    async def close(self) -> None:
        """Sends a close frame (best effort) and drops the connection. Safe
        to call more than once."""
        if self.closed and self._reader_task.done():
            return
        if not self.closed:
            try:
                await self._send_frame(OPCODE_CLOSE, encode_close_payload())
            except OSError:
                pass
        self.closed = True
        self._reader_task.cancel()
        self._writer.close()
        try:
            await self._writer.wait_closed()
        except (OSError, asyncio.CancelledError):
            pass
        try:
            await self._reader_task
        except (asyncio.CancelledError, Exception):
            pass

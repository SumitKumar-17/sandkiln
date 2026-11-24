"""`asyncio` mirror of `streams.py`: `AsyncPtySession` and `AsyncLogStream`
over `_ws_async.AsyncWebSocketConnection`. Same names, arguments and return
shapes as the sync classes, `async def` where they block. The dataclasses
and pure helpers (`ExecStreamSession`, `LogOutput`, `parse_exit_notice`)
are the sync module's, since there is nothing sync about them."""

from __future__ import annotations

from typing import AsyncIterator

from ._ws_async import AsyncWebSocketConnection
from .streams import LogOutput, parse_exit_notice


class _AsyncStream:
    def __init__(self, connection: AsyncWebSocketConnection) -> None:
        self._connection = connection

    @property
    def closed(self) -> bool:
        return self._connection.closed

    async def recv(self, timeout: float | None = None) -> bytes | str | None:
        """The next message (`bytes`, or `str` for a daemon notice), or
        `None` once the session ended. `timeout` is in seconds and raises
        `TimeoutError`; the session stays usable."""
        return await self._connection.recv(timeout)

    def __aiter__(self) -> AsyncIterator[bytes | str]:
        return self._iterate()

    async def _iterate(self) -> AsyncIterator[bytes | str]:
        while True:
            message = await self.recv()
            if message is None:
                return
            yield message

    async def close(self) -> None:
        await self._connection.close()

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc_info: object) -> None:
        await self.close()


class AsyncPtySession(_AsyncStream):
    """Async counterpart to `streams.PtySession`."""

    async def send(self, data: bytes | str) -> None:
        await self._connection.send(data)


class AsyncLogStream(_AsyncStream):
    """Async counterpart to `streams.LogStream`."""

    def __init__(self, connection: AsyncWebSocketConnection) -> None:
        super().__init__(connection)
        self.exit_code: int | None = None

    async def recv(self, timeout: float | None = None) -> bytes | str | None:
        message = await super().recv(timeout)
        if isinstance(message, str):
            code = parse_exit_notice(message)
            if code is not None:
                self.exit_code = code
        return message

    async def read_all(self) -> LogOutput:
        chunks: list[bytes] = []
        notices: list[str] = []
        async for message in self:
            if isinstance(message, str):
                notices.append(message)
            else:
                chunks.append(message)
        return LogOutput(b"".join(chunks), notices, self.exit_code)

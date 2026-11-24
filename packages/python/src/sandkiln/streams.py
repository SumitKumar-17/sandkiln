"""Live, bidirectional-or-tailing sessions over a WebSocket: `PtySession`
(`Sandbox.pty()`) and `LogStream` (`Sandbox.attach_logs()`), plus the typed
`ExecStreamSession` from `Sandbox.list_exec_streams()`. The framing lives in
`_ws.py`; `astreams.py` is the `asyncio` mirror and reuses every pure
helper and dataclass here.

The JS SDK hands back a native `WebSocket`; the standard library has none,
so these classes expose the same thing as a small blocking API: `send()`,
`recv()`, iteration, and `close()`.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Iterator
from urllib.parse import quote

from ._ws import WebSocketConnection

DEFAULT_COLS = 80
DEFAULT_ROWS = 24

# The notice the daemon sends as a text message once the process is done
# (`routes/logs.rs` `stream_logs`).
_EXIT_NOTICE = re.compile(r"^\[process exited with code (-?\d+)\]")


@dataclass(frozen=True)
class ExecStreamSession:
    """One background exec session (`Sandbox.exec_stream()` /
    `list_exec_streams()`). `exit_code` is `None` while still running."""

    id: str
    command: str
    args: list[str]
    started_at: datetime
    exit_code: int | None


@dataclass(frozen=True)
class LogOutput:
    """Everything a `LogStream` delivered up to the process exiting:
    `data` is the concatenated output bytes (stdout and stderr interleaved),
    `notices` are the daemon's text messages (truncation or exit notices),
    `exit_code` is parsed from the exit notice, `None` if the stream ended
    without one."""

    data: bytes
    notices: list[str] = field(default_factory=list)
    exit_code: int | None = None


def exec_stream_session_from_body(body: dict[str, Any]) -> ExecStreamSession:
    return ExecStreamSession(
        id=body["id"],
        command=body["command"],
        args=list(body["args"]),
        started_at=datetime.fromtimestamp(body["started_at_unix"], tz=timezone.utc),
        exit_code=body["exit_code"],
    )


def parse_exit_notice(text: str) -> int | None:
    """The exit code from the daemon's `[process exited with code N]`
    notice, or `None` if `text` is any other message."""
    match = _EXIT_NOTICE.match(text)
    return int(match.group(1)) if match else None


def pty_path(sandbox_id: str, cols: int, rows: int) -> str:
    """`/sandboxes/:id/pty` with the terminal size as query parameters. The
    daemon applies them once at session start; there is no live resize."""
    for name, value in (("cols", cols), ("rows", rows)):
        if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 0xFFFF:
            raise ValueError(f"invalid terminal {name}: {value!r}")
    return f"/sandboxes/{quote(sandbox_id, safe='')}/pty?cols={cols}&rows={rows}"


def logs_path(sandbox_id: str, session_id: str) -> str:
    # Quoted: the id lands in the handshake's request line, where a stray
    # space or CR/LF would otherwise change the request.
    return f"/sandboxes/{quote(sandbox_id, safe='')}/exec-stream/{quote(session_id, safe='')}/logs"


class _Stream:
    def __init__(self, connection: WebSocketConnection) -> None:
        self._connection = connection

    @property
    def closed(self) -> bool:
        return self._connection.closed

    def recv(self, timeout: float | None = None) -> bytes | str | None:
        """The next message (`bytes`, or `str` for a daemon notice), or
        `None` once the session ended. `timeout` is in seconds and raises
        `TimeoutError`; the session stays usable."""
        return self._connection.recv(timeout)

    def __iter__(self) -> Iterator[bytes | str]:
        while True:
            message = self.recv()
            if message is None:
                return
            yield message

    def close(self) -> None:
        self._connection.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc_info: object) -> None:
        self.close()


class PtySession(_Stream):
    """A live interactive shell, returned by `Sandbox.pty()`. What you
    `send()` becomes the shell's stdin; every message received is its
    interleaved stdout and stderr as `bytes`. Use as a context manager, or
    call `close()`: closing ends the shell and anything it started."""

    def send(self, data: bytes | str) -> None:
        self._connection.send(data)


class LogStream(_Stream):
    """A live tail of one `exec_stream()` session, returned by
    `Sandbox.attach_logs()`: the full captured history first, then new
    output as it appears. Messages are `bytes` of output, or `str` for the
    daemon's notices (the last one is the exit notice). The daemon buffers,
    so attaching again later replays the same history. `exit_code` is set
    once the exit notice has been seen."""

    def __init__(self, connection: WebSocketConnection) -> None:
        super().__init__(connection)
        self.exit_code: int | None = None

    def recv(self, timeout: float | None = None) -> bytes | str | None:
        message = super().recv(timeout)
        if isinstance(message, str):
            code = parse_exit_notice(message)
            if code is not None:
                self.exit_code = code
        return message

    def read_all(self) -> LogOutput:
        """Reads until the stream ends (the process exited, or the daemon
        closed it) and returns everything as a `LogOutput`."""
        chunks: list[bytes] = []
        notices: list[str] = []
        for message in self:
            if isinstance(message, str):
                notices.append(message)
            else:
                chunks.append(message)
        return LogOutput(b"".join(chunks), notices, self.exit_code)

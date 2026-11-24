"""The RFC 6455 pieces every WebSocket feature in this package shares:
handshake construction and validation, frame encoding/decoding, message
reassembly, and the blocking client used by `Sandbox.pty()`,
`Sandbox.attach_logs()` and the tunnel. The standard library has no
WebSocket client, so this is hand-rolled to keep the package free of
runtime dependencies (`_ws_async.py` is the `asyncio` counterpart and
imports every pure helper from here).

Everything that does no I/O (`encode_frame`, `apply_mask`,
`MessageAssembler`, `build_handshake_request`, `check_handshake`) is
separate from the socket plumbing so it can be tested without a network.
"""

from __future__ import annotations

import base64
import hashlib
import json
import os
import select
import socket
import struct
import threading
from typing import Any
from urllib.parse import urlencode, urlsplit

from .errors import SandkilnApiError

# RFC 6455 handshake constant, fixed by the spec.
_WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

OPCODE_CONTINUATION = 0x0
OPCODE_TEXT = 0x1
OPCODE_BINARY = 0x2
OPCODE_CLOSE = 0x8
OPCODE_PING = 0x9
OPCODE_PONG = 0xA

# A peer-declared length is untrusted: refuse to allocate for an absurd
# one rather than trust a 64-bit field. The daemon sends a log replay as
# one message, bounded by its own buffer cap, which is far below this.
MAX_MESSAGE_BYTES = 64 * 1024 * 1024

CLOSE_NORMAL = 1000


def generate_key() -> str:
    return base64.b64encode(os.urandom(16)).decode("ascii")


def expected_accept(key: str) -> str:
    return base64.b64encode(hashlib.sha1((key + _WS_GUID).encode("ascii")).digest()).decode("ascii")


def apply_mask(payload: bytes, mask_key: bytes) -> bytes:
    """XOR `payload` with the repeating 4-byte `mask_key`. Masking and
    unmasking are the same operation. Done as one big-integer XOR rather
    than a per-byte Python loop, which is far too slow for a log replay."""
    length = len(payload)
    if length == 0:
        return b""
    repeated = (mask_key * (length // 4 + 1))[:length]
    return (int.from_bytes(payload, "big") ^ int.from_bytes(repeated, "big")).to_bytes(length, "big")


def encode_frame(opcode: int, payload: bytes, mask_key: bytes | None = None) -> bytes:
    """One final (unfragmented) client frame. Always masked, as RFC 6455
    requires of every client-to-server frame; `mask_key` is only injectable
    so tests can pin it."""
    key = os.urandom(4) if mask_key is None else mask_key
    length = len(payload)
    first = 0x80 | opcode
    if length <= 125:
        header = bytes([first, 0x80 | length])
    elif length <= 0xFFFF:
        header = bytes([first, 0x80 | 126]) + struct.pack(">H", length)
    else:
        header = bytes([first, 0x80 | 127]) + struct.pack(">Q", length)
    return header + key + apply_mask(payload, key)


def parse_frame_head(first_two: bytes) -> tuple[bool, int, bool, int]:
    """`(fin, opcode, masked, length_code)` from a frame's first two bytes.
    `length_code` is the 7-bit field: 126 and 127 mean an extended length
    follows."""
    return (first_two[0] & 0x80) != 0, first_two[0] & 0x0F, (first_two[1] & 0x80) != 0, first_two[1] & 0x7F


def encode_close_payload(code: int = CLOSE_NORMAL) -> bytes:
    return struct.pack(">H", code)


class MessageAssembler:
    """Turns frames into messages. A data message may arrive as several
    frames (a first frame plus continuations); control frames (ping, pong,
    close) may be interleaved between them and are passed straight through.
    `feed` returns `(opcode, payload)` once a message or control frame is
    complete, else `None`. A protocol violation raises `ConnectionError`."""

    def __init__(self) -> None:
        self._opcode: int | None = None
        self._parts: list[bytes] = []
        self._size = 0

    def feed(self, fin: bool, opcode: int, payload: bytes) -> tuple[int, bytes] | None:
        if opcode >= OPCODE_CLOSE:
            if not fin or len(payload) > 125:
                raise ConnectionError("websocket protocol error: fragmented or oversized control frame")
            return opcode, payload
        if opcode == OPCODE_CONTINUATION:
            if self._opcode is None:
                raise ConnectionError("websocket protocol error: continuation frame with nothing to continue")
        else:
            if self._opcode is not None:
                raise ConnectionError("websocket protocol error: new data frame inside a fragmented message")
            self._opcode = opcode
        self._size += len(payload)
        if self._size > MAX_MESSAGE_BYTES:
            raise ConnectionError("websocket message exceeds the size limit")
        self._parts.append(payload)
        if not fin:
            return None
        message = (self._opcode, b"".join(self._parts))
        self._opcode, self._parts, self._size = None, [], 0
        return message


def decode_message(opcode: int, payload: bytes) -> bytes | str:
    """A text message as `str`, a binary one as `bytes`."""
    if opcode == OPCODE_TEXT:
        return payload.decode("utf-8", errors="replace")
    return payload


def ws_path(path: str, auth_token: str | None) -> str:
    """`path` with the token appended as `?token=`. A WebSocket handshake
    from a browser cannot carry an `Authorization` header, so the daemon's
    WebSocket routes accept the token in the query (`require_preview_token`);
    this client uses the same path so there is one mechanism, not two."""
    if auth_token is None:
        return path
    separator = "&" if "?" in path else "?"
    return f"{path}{separator}{urlencode({'token': auth_token})}"


def build_handshake_request(host: str, port: int, full_path: str, key: str) -> bytes:
    lines = [
        f"GET {full_path} HTTP/1.1",
        f"Host: {host}:{port}",
        "Upgrade: websocket",
        "Connection: Upgrade",
        f"Sec-WebSocket-Key: {key}",
        "Sec-WebSocket-Version: 13",
        "",
        "",
    ]
    return "\r\n".join(lines).encode("ascii")


def parse_status_code(status_line: str) -> int:
    parts = status_line.split(" ", 2)
    try:
        return int(parts[1])
    except (IndexError, ValueError):
        raise ConnectionError(f"websocket handshake failed: malformed status line {status_line!r}") from None


def parse_header_line(line: bytes) -> tuple[str, str] | None:
    if b":" not in line:
        return None
    name, _, value = line.partition(b":")
    return name.strip().lower().decode("latin-1"), value.strip().decode("latin-1")


def error_from_response(status: int, body: bytes) -> SandkilnApiError:
    """The daemon rejects a bad WebSocket request (unknown sandbox, session
    cap, bad token) with an ordinary JSON error before upgrading; surface it
    as the same typed error every other call raises."""
    try:
        parsed: Any = json.loads(body)
        if isinstance(parsed, dict) and isinstance(parsed.get("error"), str):
            return SandkilnApiError(status, parsed["error"], parsed)
    except (json.JSONDecodeError, UnicodeDecodeError):
        pass
    return SandkilnApiError(status, f"websocket handshake rejected with status {status}")


def check_accept(key: str, accept_value: str | None) -> None:
    if accept_value != expected_accept(key):
        raise ConnectionError("websocket handshake failed: Sec-WebSocket-Accept did not match the expected value")


class BufferedSocket:
    """Minimal buffered reader over a blocking socket: the HTTP handshake
    is read line by line, then exact byte counts for frames, carrying over
    whatever is left in the buffer between the two phases (the server's
    first frame can arrive in the same segment as the end of its handshake
    response)."""

    def __init__(self, sock: socket.socket) -> None:
        self._sock = sock
        self._buf = b""

    def has_buffered(self) -> bool:
        return bool(self._buf)

    def read_line(self) -> bytes:
        while b"\r\n" not in self._buf:
            chunk = self._sock.recv(4096)
            if not chunk:
                raise ConnectionError("socket closed while reading a line")
            self._buf += chunk
        line, _, rest = self._buf.partition(b"\r\n")
        self._buf = rest
        return line

    def read_exact(self, n: int) -> bytes:
        while len(self._buf) < n:
            chunk = self._sock.recv(max(4096, n - len(self._buf)))
            if not chunk:
                raise ConnectionError("socket closed while reading data")
            self._buf += chunk
        data, self._buf = self._buf[:n], self._buf[n:]
        return data


def connect_websocket(base_url: str, path: str, auth_token: str | None) -> tuple[socket.socket, BufferedSocket]:
    parsed = urlsplit(base_url)
    is_tls = parsed.scheme in ("https", "wss")
    host = parsed.hostname
    if host is None:
        raise ValueError(f"base_url has no host: {base_url!r}")
    port = parsed.port or (443 if is_tls else 80)

    raw_sock = socket.create_connection((host, port), timeout=10)
    sock: socket.socket = raw_sock
    try:
        if is_tls:
            import ssl

            sock = ssl.create_default_context().wrap_socket(raw_sock, server_hostname=host)
        buffered = BufferedSocket(sock)
        key = generate_key()
        sock.sendall(build_handshake_request(host, port, ws_path(path, auth_token), key))

        status = parse_status_code(buffered.read_line().decode("ascii", errors="replace"))
        headers: dict[str, str] = {}
        while True:
            line = buffered.read_line()
            if not line:
                break
            parsed_header = parse_header_line(line)
            if parsed_header is not None:
                headers[parsed_header[0]] = parsed_header[1]
        if status != 101:
            length = min(int(headers.get("content-length", "0") or 0), 65536)
            raise error_from_response(status, buffered.read_exact(length) if length else b"")
        check_accept(key, headers.get("sec-websocket-accept"))
    except BaseException:
        sock.close()
        raise
    sock.settimeout(None)
    return sock, buffered


def read_frame(buffered: BufferedSocket) -> tuple[bool, int, bytes]:
    fin, opcode, masked, length = parse_frame_head(buffered.read_exact(2))
    if length == 126:
        length = struct.unpack(">H", buffered.read_exact(2))[0]
    elif length == 127:
        length = struct.unpack(">Q", buffered.read_exact(8))[0]
    if length > MAX_MESSAGE_BYTES:
        raise ConnectionError("websocket frame exceeds the size limit")
    mask_key = buffered.read_exact(4) if masked else None
    payload = buffered.read_exact(length)
    if mask_key is not None:
        payload = apply_mask(payload, mask_key)
    return fin, opcode, payload


class WebSocketConnection:
    """A connected blocking WebSocket. `recv()` yields whole messages
    (`bytes` for binary, `str` for text) and answers pings itself; `send()`
    is safe to call from several threads. Not a general-purpose client: it
    exists for the daemon's own routes."""

    def __init__(self, sock: socket.socket, buffered: BufferedSocket) -> None:
        self._sock = sock
        self._buffered = buffered
        self._assembler = MessageAssembler()
        self._send_lock = threading.Lock()
        self.closed = False

    @classmethod
    def connect(cls, base_url: str, path: str, auth_token: str | None) -> WebSocketConnection:
        sock, buffered = connect_websocket(base_url, path, auth_token)
        return cls(sock, buffered)

    def send(self, data: bytes | str) -> None:
        if isinstance(data, str):
            self._send_frame(OPCODE_TEXT, data.encode("utf-8"))
        else:
            self._send_frame(OPCODE_BINARY, bytes(data))

    def _send_frame(self, opcode: int, payload: bytes) -> None:
        with self._send_lock:
            self._sock.sendall(encode_frame(opcode, payload))

    def recv(self, timeout: float | None = None) -> bytes | str | None:
        """The next message, or `None` once the peer closed. With a
        `timeout` (seconds), raises `TimeoutError` if nothing arrives in time;
        the connection stays usable. The wait is for the first byte only, so a
        timeout never strands a half-read frame."""
        while not self.closed:
            if timeout is not None and not self._buffered.has_buffered() and not self._readable(timeout):
                raise TimeoutError("no websocket message within the timeout")
            try:
                fin, opcode, payload = read_frame(self._buffered)
            except (ConnectionError, OSError):
                self._mark_closed()
                return None
            event = self._assembler.feed(fin, opcode, payload)
            if event is None:
                continue
            kind, data = event
            if kind == OPCODE_PING:
                try:
                    self._send_frame(OPCODE_PONG, data)
                except OSError:
                    self._mark_closed()
                    return None
            elif kind == OPCODE_CLOSE:
                try:
                    self._send_frame(OPCODE_CLOSE, data[:2])
                except OSError:
                    pass
                self._mark_closed()
                return None
            elif kind != OPCODE_PONG:
                return decode_message(kind, data)
        return None

    def _readable(self, timeout: float) -> bool:
        pending = getattr(self._sock, "pending", None)
        if pending is not None and pending() > 0:
            return True
        ready, _, _ = select.select([self._sock], [], [], timeout)
        return bool(ready)

    def _mark_closed(self) -> None:
        self.closed = True
        try:
            self._sock.close()
        except OSError:
            pass

    def close(self) -> None:
        """Sends a close frame (best effort) and drops the connection.
        Safe to call more than once, and from another thread to unblock a
        `recv()` in progress."""
        if self.closed:
            return
        self.closed = True
        try:
            self._send_frame(OPCODE_CLOSE, encode_close_payload())
        except OSError:
            pass
        try:
            self._sock.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        try:
            self._sock.close()
        except OSError:
            pass

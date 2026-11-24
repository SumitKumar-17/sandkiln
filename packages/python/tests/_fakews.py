"""A tiny in-process WebSocket server for the tests: enough of RFC 6455 to
accept the SDK's handshake, send scripted frames, and read what the client
sends back (always masked). Not part of the package."""

from __future__ import annotations

import json
import socket
import struct
import threading

from sandkiln._ws import apply_mask, expected_accept


class ServerSide:
    def __init__(self, conn: socket.socket, path: str, headers: dict[str, str]) -> None:
        self.conn = conn
        self.path = path
        self.headers = headers
        self.buf = b""

    def _need(self, n: int) -> bytes:
        while len(self.buf) < n:
            chunk = self.conn.recv(4096)
            if not chunk:
                raise ConnectionError("client closed")
            self.buf += chunk
        data, self.buf = self.buf[:n], self.buf[n:]
        return data

    def send(self, opcode: int, payload: bytes, fin: bool = True) -> None:
        first = (0x80 if fin else 0) | opcode
        n = len(payload)
        if n <= 125:
            head = bytes([first, n])
        elif n <= 0xFFFF:
            head = bytes([first, 126]) + struct.pack(">H", n)
        else:
            head = bytes([first, 127]) + struct.pack(">Q", n)
        self.conn.sendall(head + payload)

    def read(self) -> tuple[int, bytes]:
        """One client frame as `(opcode, unmasked payload)`; asserts it was masked."""
        b0, b1 = self._need(2)
        assert b1 & 0x80, "client frames must be masked"
        n = b1 & 0x7F
        if n == 126:
            n = struct.unpack(">H", self._need(2))[0]
        elif n == 127:
            n = struct.unpack(">Q", self._need(8))[0]
        key = self._need(4)
        return b0 & 0x0F, apply_mask(self._need(n), key)


class FakeWsServer:
    def __init__(self, script, reject: tuple[int, dict] | None = None) -> None:
        """`script(side)` runs once per connection after the handshake;
        `reject=(status, json_body)` answers the handshake with a plain error instead."""
        self.script = script
        self.reject = reject
        self.paths: list[str] = []
        self.errors: list[BaseException] = []
        self.sock = socket.socket()
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(4)
        self.url = f"http://127.0.0.1:{self.sock.getsockname()[1]}"
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _serve(self) -> None:
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            threading.Thread(target=self._handle, args=(conn,), daemon=True).start()

    def _handle(self, conn: socket.socket) -> None:
        try:
            data = b""
            while b"\r\n\r\n" not in data:
                chunk = conn.recv(4096)
                if not chunk:
                    return
                data += chunk
            head, _, rest = data.partition(b"\r\n\r\n")
            lines = head.decode().split("\r\n")
            path = lines[0].split(" ")[1]
            headers = {k.lower(): v.strip() for k, _, v in (ln.partition(":") for ln in lines[1:])}
            self.paths.append(path)
            if self.reject is not None:
                status, body = self.reject
                payload = json.dumps(body).encode()
                conn.sendall(
                    f"HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {len(payload)}\r\n\r\n".encode()
                    + payload
                )
                return
            accept = expected_accept(headers["sec-websocket-key"])
            conn.sendall(
                f"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n".encode()
            )
            side = ServerSide(conn, path, headers)
            side.buf = rest
            self.script(side)
        except (ConnectionError, OSError):
            pass
        except BaseException as error:  # surfaced by the test via `errors`
            self.errors.append(error)
        finally:
            try:
                conn.close()
            except OSError:
                pass

    def close(self) -> None:
        self.sock.close()

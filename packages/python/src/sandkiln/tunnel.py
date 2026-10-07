from __future__ import annotations

import base64
import hashlib
import os
import socket
import struct
import threading
from urllib.parse import urlencode, urlsplit

from ._http import request as http_request

# RFC 6455 handshake constant -- fixed by the spec, not configurable.
_WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

_OPCODE_BINARY = 0x2
_OPCODE_CLOSE = 0x8
_OPCODE_PING = 0x9
_OPCODE_PONG = 0xA

# Tunnel multiplexing frame ops -- shared wire format with the daemon's
# `routes_tunnel.rs` (`encode_frame`/`decode_frame` there) and the JS
# SDK's `tunnel.ts`. Change one, change all three.
_OP_OPEN = 0
_OP_DATA = 1
_OP_CLOSE = 2


def _generate_key() -> str:
    return base64.b64encode(os.urandom(16)).decode("ascii")


def _expected_accept(key: str) -> str:
    return base64.b64encode(hashlib.sha1((key + _WS_GUID).encode("ascii")).digest()).decode("ascii")


def _encode_ws_frame(opcode: int, payload: bytes) -> bytes:
    """One RFC 6455 frame, always masked (required for every client-to-
    server frame) and never fragmented -- every payload this module ever
    sends (an 8KiB-chunked tunnel frame at most) is far under any size
    where fragmentation would matter."""
    fin_opcode = 0x80 | opcode
    length = len(payload)
    if length <= 125:
        header = bytes([fin_opcode, 0x80 | length])
    elif length <= 0xFFFF:
        header = bytes([fin_opcode, 0x80 | 126]) + struct.pack(">H", length)
    else:
        header = bytes([fin_opcode, 0x80 | 127]) + struct.pack(">Q", length)
    mask_key = os.urandom(4)
    masked = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    return header + mask_key + masked


def _encode_tunnel_frame(op: int, conn_id: str, payload: bytes) -> bytes:
    conn_id_bytes = conn_id.encode("utf-8")
    return bytes([op, len(conn_id_bytes)]) + conn_id_bytes + payload


def _decode_tunnel_frame(data: bytes) -> tuple[int, str, bytes] | None:
    if len(data) < 2:
        return None
    op = data[0]
    length = data[1]
    if len(data) < 2 + length:
        return None
    conn_id = data[2 : 2 + length].decode("utf-8")
    return op, conn_id, data[2 + length :]


class _BufferedSocket:
    """Minimal buffered reader over a blocking socket -- just enough to
    read the HTTP handshake line-by-line and then switch to reading exact
    byte counts for WS frames, carrying over whatever's left in the
    buffer between the two phases (the server's first frame can arrive in
    the same TCP segment as the end of its handshake response)."""

    def __init__(self, sock: socket.socket) -> None:
        self._sock = sock
        self._buf = b""

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


def _connect_websocket(base_url: str, path: str, auth_token: str | None) -> tuple[socket.socket, _BufferedSocket]:
    parsed = urlsplit(base_url)
    is_tls = parsed.scheme in ("https", "wss")
    host = parsed.hostname
    if host is None:
        raise ValueError(f"base_url has no host: {base_url!r}")
    port = parsed.port or (443 if is_tls else 80)

    raw_sock = socket.create_connection((host, port), timeout=10)
    sock: socket.socket = raw_sock
    if is_tls:
        import ssl

        sock = ssl.create_default_context().wrap_socket(raw_sock, server_hostname=host)
    sock.settimeout(None)
    buffered = _BufferedSocket(sock)

    full_path = path
    if auth_token is not None:
        # Same reason the JS SDK's pty()/tunnel() append this instead of
        # a header: no WebSocket handshake path here involves a plain
        # Authorization header either, so match that convention rather
        # than inventing a second auth mechanism for this one client.
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
    sock.sendall("\r\n".join(request_lines).encode("ascii"))

    status_line = buffered.read_line().decode("ascii", errors="replace")
    if " 101 " not in status_line:
        raise ConnectionError(f"websocket handshake failed: {status_line}")

    accept_value = None
    while True:
        line = buffered.read_line()
        if not line:
            break
        if b":" in line:
            name, _, value = line.partition(b":")
            if name.strip().lower() == b"sec-websocket-accept":
                accept_value = value.strip().decode("ascii")
    if accept_value != _expected_accept(key):
        raise ConnectionError("websocket handshake failed: Sec-WebSocket-Accept did not match the expected value")

    return sock, buffered


def _read_ws_frame(buffered: _BufferedSocket) -> tuple[int, bytes]:
    first_two = buffered.read_exact(2)
    opcode = first_two[0] & 0x0F
    masked = (first_two[1] & 0x80) != 0
    length = first_two[1] & 0x7F
    if length == 126:
        length = struct.unpack(">H", buffered.read_exact(2))[0]
    elif length == 127:
        length = struct.unpack(">Q", buffered.read_exact(8))[0]
    mask_key = buffered.read_exact(4) if masked else None
    payload = buffered.read_exact(length)
    if mask_key is not None:
        payload = bytes(b ^ mask_key[i % 4] for i, b in enumerate(payload))
    return opcode, payload


class TunnelHandle:
    """A live local tunnel, returned by `Sandbox.tunnel()`. Call
    `close()` when done forwarding -- safe to call more than once."""

    def __init__(
        self,
        tunnel_id: str,
        sock: socket.socket,
        stop: threading.Event,
        thread: threading.Thread,
        base_url: str,
        sandbox_id: str,
        auth_token: str | None,
    ) -> None:
        self.tunnel_id = tunnel_id
        self._sock = sock
        self._stop = stop
        self._thread = thread
        self._base_url = base_url
        self._sandbox_id = sandbox_id
        self._auth_token = auth_token
        self._closed = False

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        self._stop.set()
        try:
            self._sock.close()
        except OSError:
            pass
        self._thread.join(timeout=2)
        try:
            http_request(self._base_url, "DELETE", f"/sandboxes/{self._sandbox_id}/tunnel/{self.tunnel_id}", self._auth_token)
        except Exception:
            # Best-effort -- the daemon-side tunnel is torn down whenever
            # it next notices the guest/socket is gone either way.
            pass


def open_tunnel(
    base_url: str,
    auth_token: str | None,
    sandbox_id: str,
    guest_port: int,
    local_port: int,
    local_host: str = "127.0.0.1",
) -> TunnelHandle:
    """Opens a local tunnel: code running inside the sandbox connects to
    `guest_port` and reaches whatever's listening on `local_port` on this
    machine -- the reverse of `preview_url`. The relay runs on a
    background thread; this call returns once the WebSocket is actually
    open, not just once it's been requested."""
    body = http_request(base_url, "POST", f"/sandboxes/{sandbox_id}/tunnel", auth_token, {"guest_port": guest_port})
    tunnel_id = body["tunnel_id"]

    ws_sock, buffered = _connect_websocket(base_url, f"/sandboxes/{sandbox_id}/tunnel/{tunnel_id}/ws", auth_token)

    stop = threading.Event()
    local_sockets: dict[str, socket.socket] = {}
    send_lock = threading.Lock()

    def send_ws(opcode: int, payload: bytes) -> None:
        with send_lock:
            ws_sock.sendall(_encode_ws_frame(opcode, payload))

    def pump_local_to_ws(conn_id: str, local_sock: socket.socket) -> None:
        try:
            while not stop.is_set():
                chunk = local_sock.recv(8192)
                if not chunk:
                    break
                send_ws(_OPCODE_BINARY, _encode_tunnel_frame(_OP_DATA, conn_id, chunk))
        except OSError:
            pass
        finally:
            local_sockets.pop(conn_id, None)
            try:
                send_ws(_OPCODE_BINARY, _encode_tunnel_frame(_OP_CLOSE, conn_id, b""))
            except OSError:
                pass

    def relay_loop() -> None:
        try:
            while not stop.is_set():
                opcode, payload = _read_ws_frame(buffered)
                if opcode == _OPCODE_CLOSE:
                    break
                if opcode == _OPCODE_PING:
                    send_ws(_OPCODE_PONG, payload)
                    continue
                if opcode != _OPCODE_BINARY:
                    continue
                decoded = _decode_tunnel_frame(payload)
                if decoded is None:
                    continue
                op, conn_id, data = decoded
                if op == _OP_OPEN:
                    try:
                        local_sock = socket.create_connection((local_host, local_port), timeout=10)
                    except OSError:
                        send_ws(_OPCODE_BINARY, _encode_tunnel_frame(_OP_CLOSE, conn_id, b""))
                        continue
                    local_sockets[conn_id] = local_sock
                    threading.Thread(target=pump_local_to_ws, args=(conn_id, local_sock), daemon=True).start()
                elif op == _OP_DATA:
                    local_sock = local_sockets.get(conn_id)
                    if local_sock is not None:
                        try:
                            local_sock.sendall(data)
                        except OSError:
                            local_sockets.pop(conn_id, None)
                elif op == _OP_CLOSE:
                    local_sock = local_sockets.pop(conn_id, None)
                    if local_sock is not None:
                        try:
                            local_sock.close()
                        except OSError:
                            pass
        except (OSError, ConnectionError):
            pass
        finally:
            for s in local_sockets.values():
                try:
                    s.close()
                except OSError:
                    pass
            local_sockets.clear()

    thread = threading.Thread(target=relay_loop, daemon=True)
    thread.start()

    return TunnelHandle(tunnel_id, ws_sock, stop, thread, base_url, sandbox_id, auth_token)

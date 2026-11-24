from __future__ import annotations

import socket
import threading

from ._http import request as http_request
from ._ws import (
    OPCODE_BINARY,
    OPCODE_CLOSE,
    OPCODE_PING,
    OPCODE_PONG,
    MessageAssembler,
    connect_websocket,
    encode_frame,
    read_frame,
)

# Tunnel multiplexing frame ops -- shared wire format with the daemon's
# `routes_tunnel.rs` (`encode_frame`/`decode_frame` there) and the JS
# SDK's `tunnel.ts`. Change one, change all three.
_OP_OPEN = 0
_OP_DATA = 1
_OP_CLOSE = 2


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

    ws_sock, buffered = connect_websocket(base_url, f"/sandboxes/{sandbox_id}/tunnel/{tunnel_id}/ws", auth_token)

    stop = threading.Event()
    local_sockets: dict[str, socket.socket] = {}
    send_lock = threading.Lock()

    def send_ws(opcode: int, payload: bytes) -> None:
        with send_lock:
            ws_sock.sendall(encode_frame(opcode, payload))

    def pump_local_to_ws(conn_id: str, local_sock: socket.socket) -> None:
        try:
            while not stop.is_set():
                chunk = local_sock.recv(8192)
                if not chunk:
                    break
                send_ws(OPCODE_BINARY, _encode_tunnel_frame(_OP_DATA, conn_id, chunk))
        except OSError:
            pass
        finally:
            local_sockets.pop(conn_id, None)
            try:
                send_ws(OPCODE_BINARY, _encode_tunnel_frame(_OP_CLOSE, conn_id, b""))
            except OSError:
                pass

    def relay_loop() -> None:
        assembler = MessageAssembler()
        try:
            while not stop.is_set():
                fin, opcode, payload = read_frame(buffered)
                event = assembler.feed(fin, opcode, payload)
                if event is None:
                    continue
                opcode, payload = event
                if opcode == OPCODE_CLOSE:
                    break
                if opcode == OPCODE_PING:
                    send_ws(OPCODE_PONG, payload)
                    continue
                if opcode != OPCODE_BINARY:
                    continue
                decoded = _decode_tunnel_frame(payload)
                if decoded is None:
                    continue
                op, conn_id, data = decoded
                if op == _OP_OPEN:
                    try:
                        local_sock = socket.create_connection((local_host, local_port), timeout=10)
                    except OSError:
                        send_ws(OPCODE_BINARY, _encode_tunnel_frame(_OP_CLOSE, conn_id, b""))
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

"""Unit tests for the hand-rolled RFC 6455 client (`_ws.py`/`_ws_async.py`):
framing, masking, message reassembly, handshake helpers, and sync and async
round trips against an in-process server. Plain `test_*` functions with no
test-framework import, so they run as `python tests/test_websocket.py` and
also under pytest.
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from _fakews import FakeWsServer  # noqa: E402
from sandkiln import SandkilnApiError  # noqa: E402
from sandkiln._ws import (  # noqa: E402
    MAX_MESSAGE_BYTES,
    OPCODE_BINARY,
    OPCODE_CLOSE,
    OPCODE_CONTINUATION,
    OPCODE_PING,
    OPCODE_PONG,
    OPCODE_TEXT,
    MessageAssembler,
    WebSocketConnection,
    apply_mask,
    build_handshake_request,
    check_accept,
    encode_frame,
    error_from_response,
    expected_accept,
    parse_frame_head,
    parse_status_code,
    ws_path,
)
from sandkiln._ws_async import AsyncWebSocketConnection  # noqa: E402


def raises(exc, fn, *args):
    try:
        fn(*args)
    except exc as error:
        return error
    raise AssertionError(f"{exc.__name__} not raised")


# --- pure helpers -----------------------------------------------------------


def test_accept_matches_the_rfc_test_vector():
    # RFC 6455 section 1.3.
    assert expected_accept("dGhlIHNhbXBsZSBub25jZQ==") == "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="


def test_masked_hello_matches_the_rfc_example():
    # RFC 6455 section 5.7: a single-frame masked text message "Hello".
    frame = encode_frame(OPCODE_TEXT, b"Hello", bytes.fromhex("37fa213d"))
    assert frame == bytes.fromhex("818537fa213d7f9f4d5158")


def test_masking_is_its_own_inverse_for_every_length():
    key = bytes([1, 2, 3, 250])
    for n in (0, 1, 3, 4, 5, 125, 126, 1000):
        data = bytes(i % 251 for i in range(n))
        masked = apply_mask(data, key)
        assert len(masked) == n
        assert apply_mask(masked, key) == data
        assert masked == bytes(b ^ key[i % 4] for i, b in enumerate(data))


def test_frame_length_encodings():
    key = bytes(4)
    assert encode_frame(OPCODE_BINARY, b"x" * 125, key)[1] == 0x80 | 125
    medium = encode_frame(OPCODE_BINARY, b"x" * 126, key)
    assert medium[1] == 0x80 | 126 and medium[2:4] == (126).to_bytes(2, "big")
    large = encode_frame(OPCODE_BINARY, b"x" * 65536, key)
    assert large[1] == 0x80 | 127 and large[2:10] == (65536).to_bytes(8, "big")


def test_client_frames_are_always_masked_with_a_fresh_key():
    a, b = encode_frame(OPCODE_BINARY, b"same"), encode_frame(OPCODE_BINARY, b"same")
    assert a[1] & 0x80 and b[1] & 0x80
    assert a[2:6] != b[2:6] or a[6:] != b[6:]  # overwhelmingly likely; 2^-32 flake


def test_parse_frame_head():
    assert parse_frame_head(bytes([0x82, 0x05])) == (True, OPCODE_BINARY, False, 5)
    assert parse_frame_head(bytes([0x01, 0xFE])) == (False, OPCODE_TEXT, True, 126)


def test_assembler_passes_a_single_frame_through():
    assert MessageAssembler().feed(True, OPCODE_BINARY, b"abc") == (OPCODE_BINARY, b"abc")


def test_assembler_joins_fragments_and_lets_control_frames_interleave():
    a = MessageAssembler()
    assert a.feed(False, OPCODE_TEXT, b"he") is None
    assert a.feed(True, OPCODE_PING, b"p") == (OPCODE_PING, b"p")
    assert a.feed(False, OPCODE_CONTINUATION, b"l") is None
    assert a.feed(True, OPCODE_CONTINUATION, b"lo") == (OPCODE_TEXT, b"hello")
    # State is reset for the next message.
    assert a.feed(True, OPCODE_BINARY, b"z") == (OPCODE_BINARY, b"z")


def test_assembler_rejects_protocol_violations():
    raises(ConnectionError, MessageAssembler().feed, True, OPCODE_CONTINUATION, b"x")
    a = MessageAssembler()
    a.feed(False, OPCODE_BINARY, b"x")
    raises(ConnectionError, a.feed, True, OPCODE_BINARY, b"y")
    raises(ConnectionError, MessageAssembler().feed, False, OPCODE_PING, b"")
    raises(ConnectionError, MessageAssembler().feed, True, OPCODE_PING, b"x" * 126)


def test_assembler_caps_the_total_message_size():
    a = MessageAssembler()
    a.feed(False, OPCODE_BINARY, b"x" * (MAX_MESSAGE_BYTES // 2 + 1))
    raises(ConnectionError, a.feed, True, OPCODE_CONTINUATION, b"x" * (MAX_MESSAGE_BYTES // 2 + 1))


def test_ws_path_appends_the_token_as_a_query_parameter():
    assert ws_path("/x", None) == "/x"
    assert ws_path("/x", "tok") == "/x?token=tok"
    assert ws_path("/x?cols=80", "a b&c") == "/x?cols=80&token=a+b%26c"


def test_handshake_request_shape():
    request = build_handshake_request("h", 7777, "/p?token=t", "KEY").decode()
    assert request.startswith("GET /p?token=t HTTP/1.1\r\n")
    for line in ("Host: h:7777", "Upgrade: websocket", "Connection: Upgrade", "Sec-WebSocket-Key: KEY", "Sec-WebSocket-Version: 13"):
        assert line + "\r\n" in request
    assert request.endswith("\r\n\r\n")


def test_status_parsing_and_accept_checking():
    assert parse_status_code("HTTP/1.1 101 Switching Protocols") == 101
    raises(ConnectionError, parse_status_code, "garbage")
    check_accept("dGhlIHNhbXBsZSBub25jZQ==", "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=")
    raises(ConnectionError, check_accept, "dGhlIHNhbXBsZSBub25jZQ==", "nope")
    raises(ConnectionError, check_accept, "dGhlIHNhbXBsZSBub25jZQ==", None)


def test_rejected_handshake_becomes_a_typed_api_error():
    err = error_from_response(404, b'{"error": "sandbox x not found"}')
    assert isinstance(err, SandkilnApiError) and err.status == 404 and err.message == "sandbox x not found"
    assert error_from_response(502, b"<html>").status == 502


# --- sync client against a real socket ---------------------------------------


def echo_script(side):
    side.send(OPCODE_BINARY, b"ready")
    side.send(OPCODE_PING, b"are you there")
    while True:
        opcode, payload = side.read()
        if opcode == OPCODE_PONG:
            side.send(OPCODE_TEXT, b"pong:" + payload)
        elif opcode == OPCODE_CLOSE:
            side.send(OPCODE_CLOSE, payload[:2])
            return
        else:
            side.send(opcode, payload.upper())


def test_sync_round_trip_with_ping_text_and_close():
    server = FakeWsServer(echo_script)
    try:
        ws = WebSocketConnection.connect(server.url, "/sandboxes/abc/pty?cols=80", "secret")
        assert ws.recv(timeout=5) == b"ready"
        # The server's ping is answered automatically; its pong-reply arrives as text.
        assert ws.recv(timeout=5) == "pong:are you there"
        ws.send(b"hello")
        assert ws.recv(timeout=5) == b"HELLO"
        ws.send("text")
        assert ws.recv(timeout=5) == "TEXT"
        ws.close()
        ws.close()  # idempotent
        assert ws.recv() is None
        assert server.paths == ["/sandboxes/abc/pty?cols=80&token=secret"]
        assert not server.errors, server.errors
    finally:
        server.close()


def test_sync_recv_timeout_leaves_the_connection_usable():
    def script(side):
        side.read()  # wait for the client's first message before answering
        side.send(OPCODE_BINARY, b"late")
        side.read()

    server = FakeWsServer(script)
    try:
        ws = WebSocketConnection.connect(server.url, "/x", None)
        raises(TimeoutError, ws.recv, 0.1)
        ws.send(b"go")
        assert ws.recv(timeout=5) == b"late"
        ws.close()
    finally:
        server.close()


def test_sync_reassembles_a_fragmented_message_and_sees_server_close():
    def script(side):
        side.send(OPCODE_TEXT, b"a", fin=False)
        side.send(OPCODE_CONTINUATION, b"b", fin=False)
        side.send(OPCODE_CONTINUATION, b"c")
        side.send(OPCODE_BINARY, bytes(range(256)) * 400)  # 102400 bytes: 8-byte length form
        side.send(OPCODE_CLOSE, b"\x03\xe8")
        side.read()

    server = FakeWsServer(script)
    try:
        ws = WebSocketConnection.connect(server.url, "/x", None)
        assert ws.recv(timeout=5) == "abc"
        assert ws.recv(timeout=5) == bytes(range(256)) * 400
        assert ws.recv(timeout=5) is None
        assert ws.closed
    finally:
        server.close()


def test_sync_connect_surfaces_the_daemons_error_before_upgrade():
    server = FakeWsServer(lambda side: None, reject=(409, {"error": "sandbox x already has 64 open PTY sessions"}))
    try:
        err = raises(SandkilnApiError, WebSocketConnection.connect, server.url, "/sandboxes/x/pty", None)
        assert err.status == 409 and "PTY sessions" in err.message
    finally:
        server.close()


# --- asyncio client -----------------------------------------------------------


def test_async_round_trip_fragments_and_close():
    def script(side):
        side.send(OPCODE_BINARY, b"ready")
        side.send(OPCODE_PING, b"ping")
        assert side.read() == (OPCODE_PONG, b"ping")
        side.send(OPCODE_TEXT, b"x", fin=False)
        side.send(OPCODE_CONTINUATION, b"y")
        opcode, payload = side.read()
        side.send(opcode, payload[::-1])
        side.send(OPCODE_TEXT, b"[process exited with code 3]\n")
        side.send(OPCODE_CLOSE, b"")
        try:
            side.read()
        except ConnectionError:
            pass

    server = FakeWsServer(script)

    async def go():
        ws = await AsyncWebSocketConnection.connect(server.url, "/x", "t")
        assert await ws.recv(timeout=5) == b"ready"
        assert await ws.recv(timeout=5) == "xy"
        await ws.send(b"abc")
        assert await ws.recv(timeout=5) == b"cba"
        assert await ws.recv(timeout=5) == "[process exited with code 3]\n"
        assert await ws.recv(timeout=5) is None
        assert await ws.recv(timeout=5) is None  # stays ended
        await ws.close()
        await ws.close()

    try:
        asyncio.run(go())
        assert server.paths == ["/x?token=t"]
        assert not server.errors, server.errors
    finally:
        server.close()


def test_async_recv_timeout_and_rejected_handshake():
    def script(side):
        side.read()
        side.send(OPCODE_BINARY, b"late")
        side.read()

    server = FakeWsServer(script)
    rejecting = FakeWsServer(lambda side: None, reject=(404, {"error": "nope"}))

    async def go():
        ws = await AsyncWebSocketConnection.connect(server.url, "/x", None)
        try:
            await ws.recv(timeout=0.1)
        except TimeoutError:
            pass
        else:
            raise AssertionError("expected TimeoutError")
        await ws.send(b"go")
        assert await ws.recv(timeout=5) == b"late"
        await ws.close()
        try:
            await AsyncWebSocketConnection.connect(rejecting.url, "/x", None)
        except SandkilnApiError as error:
            assert error.status == 404
        else:
            raise AssertionError("expected SandkilnApiError")

    try:
        asyncio.run(go())
    finally:
        server.close()
        rejecting.close()


def run_all() -> int:
    failures = 0
    tests = [(n, f) for n, f in sorted(globals().items()) if n.startswith("test_") and callable(f)]
    for name, fn in tests:
        try:
            fn()
            print(f"ok   {name}")
        except BaseException as error:
            failures += 1
            print(f"FAIL {name}: {error!r}")
    print(f"{len(tests) - failures}/{len(tests)} passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(run_all())

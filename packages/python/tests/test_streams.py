"""Tests for `streams.py`/`astreams.py` and the `Sandbox`/`AsyncSandbox`
methods built on them (`pty`, `exec_stream`, `list_exec_streams`,
`attach_logs`), against in-process fake servers. Plain `test_*` functions;
run as `python tests/test_streams.py` or under pytest.
"""

from __future__ import annotations

import asyncio
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from _fakews import FakeWsServer  # noqa: E402
from sandkiln import AsyncSandbox, ExecStreamSession, LogOutput, Sandbox, SandkilnApiError  # noqa: E402
from sandkiln._ws import OPCODE_BINARY, OPCODE_CLOSE, OPCODE_TEXT  # noqa: E402
from sandkiln.streams import logs_path, parse_exit_notice, pty_path  # noqa: E402
from sandkiln.tunnel import _OP_DATA, _decode_tunnel_frame, _encode_tunnel_frame  # noqa: E402


def raises(exc, fn, *args):
    try:
        fn(*args)
    except exc as error:
        return error
    raise AssertionError(f"{exc.__name__} not raised")


def test_exit_notice_parsing():
    assert parse_exit_notice("[process exited with code 0]\n") == 0
    assert parse_exit_notice("[process exited with code 137]\n") == 137
    assert parse_exit_notice("[process exited with code -1]\n") == -1
    assert parse_exit_notice("[... 5 earlier bytes truncated ...]\n") is None
    assert parse_exit_notice("echo [process exited with code 1]") is None


def test_pty_path_validates_the_size_and_quotes_the_id():
    assert pty_path("abc", 100, 30) == "/sandboxes/abc/pty?cols=100&rows=30"
    for cols, rows in ((0, 24), (80, 0), (70000, 24), (True, 24), (80, "24"), (-1, 24)):
        raises(ValueError, pty_path, "abc", cols, rows)
    assert " " not in pty_path("a b\r\nX: y", 80, 24) and "\r" not in pty_path("a b\r\nX: y", 80, 24)


def test_logs_path_quotes_ids_so_they_cannot_inject_request_lines():
    assert logs_path("sb", "s1") == "/sandboxes/sb/exec-stream/s1/logs"
    hostile = logs_path("sb", "x HTTP/1.1\r\nX-Evil: 1")
    assert "\r" not in hostile and "\n" not in hostile and " " not in hostile


def test_tunnel_frame_round_trip_survives_the_refactor():
    frame = _encode_tunnel_frame(_OP_DATA, "c1", b"payload")
    assert frame == bytes([_OP_DATA, 2]) + b"c1payload"
    assert _decode_tunnel_frame(frame) == (_OP_DATA, "c1", b"payload")
    assert _decode_tunnel_frame(b"\x01") is None


def pty_script(side):
    side.send(OPCODE_BINARY, b"$ ")
    opcode, payload = side.read()
    side.send(OPCODE_BINARY, payload)  # echo
    side.read()  # the client's close


def test_sandbox_pty_round_trip_sends_size_and_token():
    server = FakeWsServer(pty_script)
    try:
        with Sandbox.attach("sb1", base_url=server.url, auth_token="tok").pty(cols=120, rows=40) as pty:
            assert pty.recv(timeout=5) == b"$ "
            pty.send("echo hi\n")
            assert pty.recv(timeout=5) == b"echo hi\n"
        assert pty.closed
        assert server.paths == ["/sandboxes/sb1/pty?cols=120&rows=40&token=tok"]
    finally:
        server.close()


def test_sandbox_pty_surfaces_a_refused_session():
    server = FakeWsServer(lambda s: None, reject=(404, {"error": "sandbox sb1 not found"}))
    try:
        err = raises(SandkilnApiError, Sandbox.attach("sb1", base_url=server.url).pty)
        assert err.status == 404
    finally:
        server.close()


def logs_script(side):
    side.send(OPCODE_TEXT, b"[... 10 earlier bytes truncated ...]\n")
    side.send(OPCODE_BINARY, b"line 1\n")
    side.send(OPCODE_BINARY, b"line 2\n")
    side.send(OPCODE_TEXT, b"[process exited with code 7]\n")
    side.send(OPCODE_CLOSE, b"")
    try:
        side.read()
    except ConnectionError:
        pass


def test_attach_logs_collects_output_notices_and_exit_code():
    server = FakeWsServer(logs_script)
    try:
        stream = Sandbox.attach("sb1", base_url=server.url).attach_logs("sess1")
        out = stream.read_all()
        assert out == LogOutput(b"line 1\nline 2\n", ["[... 10 earlier bytes truncated ...]\n", "[process exited with code 7]\n"], 7)
        assert stream.exit_code == 7 and stream.recv() is None
        assert server.paths == ["/sandboxes/sb1/exec-stream/sess1/logs"]
    finally:
        server.close()


def test_async_attach_logs_and_pty():
    logs = FakeWsServer(logs_script)
    pty = FakeWsServer(pty_script)

    async def go():
        sandbox = AsyncSandbox.attach("sb1", base_url=logs.url, auth_token="t")
        async with await sandbox.attach_logs("sess1") as stream:
            out = await stream.read_all()
        assert out.data == b"line 1\nline 2\n" and out.exit_code == 7 and stream.exit_code == 7
        async with await AsyncSandbox.attach("sb1", base_url=pty.url).pty(cols=90) as session:
            assert await session.recv(timeout=5) == b"$ "
            await session.send(b"ls\n")
            seen = []
            async for message in session:
                seen.append(message)
                break
            assert seen == [b"ls\n"]

    try:
        asyncio.run(go())
        assert logs.paths == ["/sandboxes/sb1/exec-stream/sess1/logs?token=t"]
        assert pty.paths == ["/sandboxes/sb1/pty?cols=90&rows=24"]
    finally:
        logs.close()
        pty.close()


class FakeHttp:
    def __init__(self):
        outer = self
        self.calls: list[tuple[str, str, dict | None]] = []

        class Handler(BaseHTTPRequestHandler):
            def _reply(self, body):
                payload = json.dumps(body).encode()
                self.send_response(200)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                outer.calls.append(("POST", self.path, body))
                self._reply({"id": "sess9"})

            def do_GET(self):
                outer.calls.append(("GET", self.path, None))
                self._reply(
                    {
                        "sessions": [
                            {"id": "a", "command": "sleep", "args": ["5"], "started_at_unix": 1700000000, "exit_code": None},
                            {"id": "b", "command": "true", "args": [], "started_at_unix": 1700000001, "exit_code": 0},
                        ]
                    }
                )

            def log_message(self, *args):
                pass

        self.server = HTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


def test_exec_stream_and_listing_use_the_daemons_routes():
    http = FakeHttp()
    try:
        sandbox = Sandbox.attach("sb1", base_url=http.url)
        assert sandbox.exec_stream("sleep", ["5"], env={"A": "1"}) == "sess9"
        assert sandbox.exec_stream("true") == "sess9"
        sessions = sandbox.list_exec_streams()
        assert http.calls[0] == ("POST", "/sandboxes/sb1/exec-stream", {"command": "sleep", "args": ["5"], "env": {"A": "1"}})
        assert http.calls[1][2] == {"command": "true", "args": []}  # no env key unless given
        assert http.calls[2][:2] == ("GET", "/sandboxes/sb1/exec-stream")
        assert [s.exit_code for s in sessions] == [None, 0]
        assert isinstance(sessions[0], ExecStreamSession)
        assert sessions[0].args == ["5"] and sessions[0].started_at.year == 2023 and sessions[0].started_at.tzinfo is not None

        async def go():
            a = AsyncSandbox.attach("sb1", base_url=http.url)
            assert await a.exec_stream("true") == "sess9"
            return await a.list_exec_streams()

        assert [s.id for s in asyncio.run(go())] == ["a", "b"]
    finally:
        http.close()


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

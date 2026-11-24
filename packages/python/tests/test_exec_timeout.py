import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

import pytest

from sandkiln import AsyncSandbox, Sandbox, SandkilnApiError, SandkilnExecTimeoutError
from sandkiln.errors import as_exec_timeout

TIMED_OUT = {
    "error": "command timed out after 2s and was killed",
    "timed_out": True,
    "timeout_secs": 2,
    "stdout": "partial\n",
    "stderr": "warn\n",
}


class FakeDaemon:
    """A daemon stand-in that records the last exec body and replies with a fixed answer."""

    def __init__(self, status: int, body: dict):
        outer = self
        self.seen: dict = {}

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                outer.seen = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                payload = json.dumps(body).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                pass

        self.server = HTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()


@pytest.fixture
def daemon():
    made = []

    def make(status, body):
        d = FakeDaemon(status, body)
        made.append(d)
        return d

    yield make
    for d in made:
        d.close()


OK = {"stdout": "", "stderr": "", "exit_code": 0}


def test_timeout_is_sent_as_timeout_secs_and_omitted_when_unset(daemon):
    d = daemon(200, OK)
    sandbox = Sandbox.attach("abc", base_url=d.url)
    sandbox.run_command("true", timeout=7)
    assert d.seen["timeout_secs"] == 7
    sandbox.run_command("true")
    assert "timeout_secs" not in d.seen


def test_a_killed_command_raises_with_partial_output(daemon):
    d = daemon(504, TIMED_OUT)
    with pytest.raises(SandkilnExecTimeoutError) as caught:
        Sandbox.attach("abc", base_url=d.url).run_command("sleep", ["30"], timeout=2)
    err = caught.value
    assert isinstance(err, SandkilnApiError) and err.status == 504
    assert (err.stdout, err.stderr, err.timeout_secs) == ("partial\n", "warn\n", 2)


def test_a_bare_504_stays_a_plain_api_error(daemon):
    d = daemon(504, {"error": "no reply from the sandbox within 12s"})
    with pytest.raises(SandkilnApiError) as caught:
        Sandbox.attach("abc", base_url=d.url).run_command("sleep", ["30"], timeout=2)
    assert not isinstance(caught.value, SandkilnExecTimeoutError)


def test_async_run_command_raises_the_same_typed_error(daemon):
    d = daemon(504, TIMED_OUT)

    async def go():
        await AsyncSandbox.attach("abc", base_url=d.url).run_command("sleep", ["30"], timeout=2)

    with pytest.raises(SandkilnExecTimeoutError) as caught:
        asyncio.run(go())
    assert caught.value.stdout == "partial\n"
    assert d.seen["timeout_secs"] == 2


def test_as_exec_timeout_ignores_other_statuses():
    assert as_exec_timeout(SandkilnApiError(500, "x", dict(TIMED_OUT))) is None
    assert as_exec_timeout(SandkilnApiError(504, "x", None)) is None


def test_a_command_that_could_not_start_is_a_normal_result(daemon):
    d = daemon(200, {"stdout": "", "stderr": "nosuch: command not found\n", "exit_code": 127})
    result = Sandbox.attach("abc", base_url=d.url).run_command("nosuch")
    assert (result.exit_code, result.stderr) == (127, "nosuch: command not found\n")


def test_async_command_that_could_not_start_is_a_normal_result(daemon):
    d = daemon(200, {"stdout": "", "stderr": "/tmp: Is a directory\n", "exit_code": 126})

    async def go():
        return await AsyncSandbox.attach("abc", base_url=d.url).run_command("/tmp")

    result = asyncio.run(go())
    assert (result.exit_code, result.stderr) == (126, "/tmp: Is a directory\n")

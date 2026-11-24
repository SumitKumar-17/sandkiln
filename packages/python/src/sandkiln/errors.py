from __future__ import annotations

from typing import Any


class SandkilnApiError(Exception):
    """Raised for any non-2xx response from the sandkiln daemon."""

    def __init__(self, status: int, message: str, body: Any = None):
        super().__init__(f"{message} (status {status})")
        self.status = status
        self.message = message
        # The parsed JSON error body when the daemon sent one.
        self.body = body


class SandkilnExecTimeoutError(SandkilnApiError):
    """`run_command` hit its `timeout`: the guest killed the command's whole
    process group. `stdout`/`stderr` hold what it had printed by then.

    A subclass of `SandkilnApiError` (status 504), so code that already
    catches that keeps working."""

    def __init__(self, message: str, stdout: str, stderr: str, timeout_secs: int, body: Any = None):
        super().__init__(504, message, body)
        self.stdout = stdout
        self.stderr = stderr
        self.timeout_secs = timeout_secs


def as_exec_timeout(error: SandkilnApiError) -> SandkilnExecTimeoutError | None:
    """The daemon's "command killed at its limit" 504 as a typed error, or
    `None` for any other error (including a bare 504 from a reply that never
    came)."""
    body = error.body
    if error.status != 504 or not isinstance(body, dict) or body.get("timed_out") is not True:
        return None
    return SandkilnExecTimeoutError(
        error.message,
        str(body.get("stdout", "")),
        str(body.get("stderr", "")),
        int(body.get("timeout_secs", 0)),
        body,
    )

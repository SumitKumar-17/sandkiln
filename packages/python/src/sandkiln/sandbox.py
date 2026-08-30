from __future__ import annotations

import base64
from dataclasses import dataclass, field
from datetime import datetime, timezone
from urllib.parse import urlencode

from ._config import resolve_auth_token, resolve_base_url
from ._http import request


@dataclass(frozen=True)
class ExecResult:
    stdout: str
    stderr: str
    exit_code: int


@dataclass(frozen=True)
class SandboxInfo:
    id: str
    created_at: datetime
    tags: dict[str, str] = field(default_factory=dict)


class Sandbox:
    """A handle to one sandkiln sandbox. Construct via `Sandbox.create()`
    or `Sandbox.attach()`, not directly."""

    def __init__(self, id: str, base_url: str, auth_token: str | None):
        self.id = id
        self._base_url = base_url
        self._auth_token = auth_token

    @classmethod
    def create(
        cls,
        tags: dict[str, str] | None = None,
        base_url: str | None = None,
        auth_token: str | None = None,
        vcpu_count: int | None = None,
        mem_size_mib: int | None = None,
    ) -> "Sandbox":
        """`vcpu_count`/`mem_size_mib` override the daemon's configured
        defaults for this one sandbox; omitted (the default) uses them
        unchanged. The daemon rejects a value of `0` or anything above its
        configured ceiling (`SANDKILN_MAX_VCPU_COUNT`/
        `SANDKILN_MAX_MEM_SIZE_MIB`) with a 400."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        body: dict[str, object] = {}
        if tags is not None:
            body["tags"] = tags
        if vcpu_count is not None:
            body["vcpu_count"] = vcpu_count
        if mem_size_mib is not None:
            body["mem_size_mib"] = mem_size_mib
        response = request(resolved_base_url, "POST", "/sandboxes", resolved_token, body or None)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    def attach(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> "Sandbox":
        """Wraps an already-existing sandbox id without a network
        round-trip — for a process that only has an id from elsewhere and
        needs a handle to call instance methods on."""
        return cls(id, resolve_base_url(base_url), resolve_auth_token(auth_token))

    @classmethod
    def list(
        cls,
        tags: dict[str, str] | None = None,
        base_url: str | None = None,
        auth_token: str | None = None,
    ) -> list[SandboxInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        query = urlencode({f"tag.{k}": v for k, v in (tags or {}).items()})
        path = f"/sandboxes?{query}" if query else "/sandboxes"
        response = request(resolved_base_url, "GET", path, resolved_token)
        return [
            SandboxInfo(
                id=summary["id"],
                created_at=datetime.fromtimestamp(summary["created_at_unix"], tz=timezone.utc),
                tags=summary["tags"],
            )
            for summary in response["sandboxes"]
        ]

    def run_command(self, command: str, args: list[str] | None = None) -> ExecResult:
        body = {"command": command, "args": args or []}
        response = self._request("POST", f"/sandboxes/{self.id}/exec", body)
        return ExecResult(response["stdout"], response["stderr"], response["exit_code"])

    def read_file(self, path: str) -> bytes:
        response = self._request("POST", f"/sandboxes/{self.id}/read-file", {"path": path})
        return base64.b64decode(response["content_base64"])

    def write_file(self, path: str, content: str | bytes) -> None:
        raw = content.encode("utf-8") if isinstance(content, str) else content
        body = {"path": path, "content_base64": base64.b64encode(raw).decode("ascii")}
        self._request("POST", f"/sandboxes/{self.id}/write-file", body)

    def stop(self) -> None:
        self._request("DELETE", f"/sandboxes/{self.id}")

    def _request(self, method: str, path: str, body=None):
        return request(self._base_url, method, path, self._auth_token, body)

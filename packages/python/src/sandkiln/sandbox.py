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
    ) -> "Sandbox":
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        body = {"tags": tags} if tags is not None else None
        response = request(resolved_base_url, "POST", "/sandboxes", resolved_token, body)
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

    def preview_url(self, port: int, path: str = "/") -> str:
        """URL a browser can open directly to reach a server listening on
        `port` inside this sandbox, proxied through the daemon's
        `/sandboxes/:id/preview/:port` route. Pure and network-free, like
        `attach` — the daemon proxies lazily on each request, so there's
        nothing to create or await up front.

        If this sandbox's client has an auth token configured, it's
        appended as a `?token=` query parameter rather than sent as a
        header: the caller of this URL is typically a browser tab or an
        `<iframe src=...>` embed, neither of which can attach an
        `Authorization` header, and the daemon's preview route accepts the
        token either way.
        """
        if isinstance(port, bool) or not isinstance(port, int) or port < 1 or port > 65535:
            raise ValueError(f"invalid preview port: {port}")
        normalized_path = path if path.startswith("/") else f"/{path}"
        suffix = f"?{urlencode({'token': self._auth_token})}" if self._auth_token else ""
        return f"{self._base_url}/sandboxes/{self.id}/preview/{port}{normalized_path}{suffix}"

    def _request(self, method: str, path: str, body=None):
        return request(self._base_url, method, path, self._auth_token, body)

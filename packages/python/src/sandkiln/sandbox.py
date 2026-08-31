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


@dataclass(frozen=True)
class SnapshotInfo:
    id: str
    source_sandbox_id: str
    created_at: datetime
    tags: dict[str, str] = field(default_factory=dict)
    # Id of the live sandbox currently forked from this snapshot, if any —
    # see `Sandbox.fork`. While set, `Sandbox.fork`/`Sandbox.resume` on
    # this snapshot id both raise `SandkilnApiError` with status 409.
    forked_into: str | None = None


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
        """A sandbox can drop out of this list on its own, not just from an
        explicit `stop()`/`snapshot()` call: if the daemon has
        `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` configured, it pauses and
        snapshots an idle sandbox automatically — same effect as a manual
        `snapshot()`. Use `Sandbox.list_snapshots(source_sandbox_id=...)`
        to find out whether a sandbox id that's no longer listed here
        turned into a snapshot, and its resulting snapshot id."""
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

    def snapshot(self) -> str:
        """Saves this sandbox's full state (memory + disk) to disk and
        stops it, returning a snapshot id. The sandbox itself stops
        existing — call `Sandbox.resume` or `Sandbox.fork` on the
        returned id to boot from it again.

        The daemon can also do this on its own, without this method ever
        being called, if `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` is
        configured and this sandbox goes idle past that timeout — see
        `Sandbox.list_snapshots`."""
        response = self._request("POST", f"/sandboxes/{self.id}/snapshot")
        return response["snapshot_id"]

    @classmethod
    def resume(cls, snapshot_id: str, base_url: str | None = None, auth_token: str | None = None) -> "Sandbox":
        """Boots a new sandbox from a snapshot, consuming it — the
        snapshot is gone afterward, and the new sandbox owns its state
        outright, the same as one from `Sandbox.create`. Use
        `Sandbox.fork` instead to boot from the same snapshot more than
        once."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = request(resolved_base_url, "POST", f"/snapshots/{snapshot_id}/resume", resolved_token)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    def fork(cls, snapshot_id: str, base_url: str | None = None, auth_token: str | None = None) -> "Sandbox":
        """Boots a new sandbox from a snapshot *without* consuming it, so
        the same snapshot can be forked or resumed again later — the
        building block for starting parallel branches off one prepared
        environment without repeating its setup cost.

        Only one live sandbox forked from a given snapshot may exist at a
        time: a fork reopens the exact rootfs file the snapshot recorded
        (and, if the original sandbox was networked, the exact tap device
        — its guest IP/MAC were frozen in at that sandbox's original
        boot), so two live forks at once would mean either two VMs
        writing the same disk image or two guests colliding on one
        IP/MAC. A second `fork()` call while an earlier fork is still
        running raises `SandkilnApiError` with status 409 until that one
        is stopped."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = request(resolved_base_url, "POST", f"/snapshots/{snapshot_id}/fork", resolved_token)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    def list_snapshots(
        cls,
        source_sandbox_id: str | None = None,
        base_url: str | None = None,
        auth_token: str | None = None,
    ) -> list[SnapshotInfo]:
        """Lists snapshots. `source_sandbox_id` narrows this to the (at
        most one) snapshot taken from that original sandbox id — the way
        to go from "the sandbox id I had" to "the snapshot it became"
        after a manual `snapshot()` or the daemon's auto-suspend made it
        disappear from `Sandbox.list()`. Omitted, this lists every
        snapshot."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        query = urlencode({"source_sandbox_id": source_sandbox_id}) if source_sandbox_id else ""
        path = f"/snapshots?{query}" if query else "/snapshots"
        response = request(resolved_base_url, "GET", path, resolved_token)
        return [
            SnapshotInfo(
                id=summary["id"],
                source_sandbox_id=summary["source_sandbox_id"],
                created_at=datetime.fromtimestamp(summary["created_at_unix"], tz=timezone.utc),
                tags=summary["tags"],
                forked_into=summary["forked_into"],
            )
            for summary in response["snapshots"]
        ]

    def _request(self, method: str, path: str, body=None):
        return request(self._base_url, method, path, self._auth_token, body)

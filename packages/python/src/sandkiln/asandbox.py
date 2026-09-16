"""`AsyncSandbox`: an `asyncio`-native mirror of `Sandbox` — every method
has the exact same name, arguments, and return shape, just `async def`
and awaited with `_http_async.request` instead of `Sandbox`'s blocking
`_http.request`. For a caller already running an event loop (an async
web framework, an async agent runner), this avoids needing a thread
pool just to use this SDK without blocking it — see `ROADMAP.md`'s
Client SDKs section for the gap this closes.

Deliberately a separate class, not one class with both sync and async
methods: the two share no state and mixing them on one class invites
calling the wrong one by accident (a sync call in an async context
blocks the event loop silently — no exception, just a stall). Shares
every dataclass (`DriveAttachment`, `ExecResult`, `MountInfo`, etc.) and
body-building helper (`_build_rate_limit`, `_build_egress`, ...) with
`sandbox.py` — those are pure data/formatting, no I/O, so there is
nothing "sync" about them to mirror."""

from __future__ import annotations

import base64
from datetime import datetime, timezone
from urllib.parse import urlencode

from ._config import resolve_auth_token, resolve_base_url
from ._http_async import request
from .sandbox import (
    DirEntry,
    DriveAttachment,
    ExecResult,
    MountInfo,
    SandboxInfo,
    SnapshotInfo,
    StopResult,
    _build_egress,
    _build_rate_limit,
    _drive_attachment_to_dict,
    _mount_info_from_response,
)


class AsyncSandbox:
    """An `asyncio`-native handle to one sandkiln sandbox. Construct via
    `AsyncSandbox.create()` or `AsyncSandbox.attach()`, not directly. See
    `Sandbox` (the sync equivalent) for the full documentation of every
    method here — the two are identical in behavior, only the calling
    convention (`await`) differs."""

    def __init__(self, id: str, base_url: str, auth_token: str | None):
        self.id = id
        self._base_url = base_url
        self._auth_token = auth_token

    @classmethod
    async def create(
        cls,
        name: str | None = None,
        tags: dict[str, str] | None = None,
        base_url: str | None = None,
        auth_token: str | None = None,
        vcpu_count: int | None = None,
        mem_size_mib: int | None = None,
        image_id: str | None = None,
        rate_limit_bandwidth_bytes_per_sec: int | None = None,
        rate_limit_ops_per_sec: int | None = None,
        drives: list[DriveAttachment] | None = None,
        env: dict[str, str] | None = None,
        egress_mode: str | None = None,
        egress_allow_cidrs: list[str] | None = None,
        egress_deny_cidrs: list[str] | None = None,
    ) -> "AsyncSandbox":
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        body: dict[str, object] = {}
        if name is not None:
            body["name"] = name
        if tags is not None:
            body["tags"] = tags
        if vcpu_count is not None:
            body["vcpu_count"] = vcpu_count
        if mem_size_mib is not None:
            body["mem_size_mib"] = mem_size_mib
        if image_id is not None:
            body["image_id"] = image_id
        rate_limit = _build_rate_limit(rate_limit_bandwidth_bytes_per_sec, rate_limit_ops_per_sec)
        if rate_limit is not None:
            body["rate_limit"] = rate_limit
        if drives is not None:
            body["drives"] = [_drive_attachment_to_dict(d) for d in drives]
        if env is not None:
            body["env"] = env
        egress = _build_egress(egress_mode, egress_allow_cidrs, egress_deny_cidrs)
        if egress is not None:
            body["egress"] = egress
        response = await request(resolved_base_url, "POST", "/sandboxes", resolved_token, body or None)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    def attach(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> "AsyncSandbox":
        """Pure and network-free, like `Sandbox.attach` — not a
        coroutine, since there's nothing to await."""
        return cls(id, resolve_base_url(base_url), resolve_auth_token(auth_token))

    @classmethod
    async def by_name(cls, name: str, base_url: str | None = None, auth_token: str | None = None) -> "AsyncSandbox":
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = await request(resolved_base_url, "GET", f"/sandboxes/by-name/{name}", resolved_token)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    async def get_or_create(
        cls,
        name: str,
        tags: dict[str, str] | None = None,
        base_url: str | None = None,
        auth_token: str | None = None,
        vcpu_count: int | None = None,
        mem_size_mib: int | None = None,
        rate_limit_bandwidth_bytes_per_sec: int | None = None,
        rate_limit_ops_per_sec: int | None = None,
        drives: list[DriveAttachment] | None = None,
        env: dict[str, str] | None = None,
        egress_mode: str | None = None,
        egress_allow_cidrs: list[str] | None = None,
        egress_deny_cidrs: list[str] | None = None,
    ) -> tuple["AsyncSandbox", bool]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        body: dict[str, object] = {"name": name}
        if tags is not None:
            body["tags"] = tags
        if vcpu_count is not None:
            body["vcpu_count"] = vcpu_count
        if mem_size_mib is not None:
            body["mem_size_mib"] = mem_size_mib
        rate_limit = _build_rate_limit(rate_limit_bandwidth_bytes_per_sec, rate_limit_ops_per_sec)
        if rate_limit is not None:
            body["rate_limit"] = rate_limit
        if drives is not None:
            body["drives"] = [_drive_attachment_to_dict(d) for d in drives]
        if env is not None:
            body["env"] = env
        egress = _build_egress(egress_mode, egress_allow_cidrs, egress_deny_cidrs)
        if egress is not None:
            body["egress"] = egress
        response = await request(resolved_base_url, "POST", "/sandboxes/get-or-create", resolved_token, body)
        return cls(response["id"], resolved_base_url, resolved_token), response["created"]

    @classmethod
    async def list(
        cls,
        tags: dict[str, str] | None = None,
        base_url: str | None = None,
        auth_token: str | None = None,
    ) -> list[SandboxInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        query = urlencode({f"tag.{k}": v for k, v in (tags or {}).items()})
        path = f"/sandboxes?{query}" if query else "/sandboxes"
        response = await request(resolved_base_url, "GET", path, resolved_token)
        return [
            SandboxInfo(
                id=summary["id"],
                created_at=datetime.fromtimestamp(summary["created_at_unix"], tz=timezone.utc),
                tags=summary["tags"],
                name=summary.get("name"),
            )
            for summary in response["sandboxes"]
        ]

    async def run_command(self, command: str, args: list[str] | None = None, env: dict[str, str] | None = None) -> ExecResult:
        body: dict[str, object] = {"command": command, "args": args or []}
        if env is not None:
            body["env"] = env
        response = await self._request("POST", f"/sandboxes/{self.id}/exec", body)
        return ExecResult(response["stdout"], response["stderr"], response["exit_code"])

    async def read_file(self, path: str) -> bytes:
        response = await self._request("POST", f"/sandboxes/{self.id}/read-file", {"path": path})
        return base64.b64decode(response["content_base64"])

    async def write_file(self, path: str, content: str | bytes) -> None:
        raw = content.encode("utf-8") if isinstance(content, str) else content
        body = {"path": path, "content_base64": base64.b64encode(raw).decode("ascii")}
        await self._request("POST", f"/sandboxes/{self.id}/write-file", body)

    async def chmod(self, path: str, mode: int) -> None:
        await self._request("POST", f"/sandboxes/{self.id}/chmod", {"path": path, "mode": mode})

    async def chown(self, path: str, uid: int, gid: int) -> None:
        await self._request("POST", f"/sandboxes/{self.id}/chown", {"path": path, "uid": uid, "gid": gid})

    async def mkdir(self, path: str, parents: bool = False) -> None:
        await self._request("POST", f"/sandboxes/{self.id}/mkdir", {"path": path, "parents": parents})

    async def rename(self, from_path: str, to_path: str) -> None:
        await self._request("POST", f"/sandboxes/{self.id}/rename", {"from": from_path, "to": to_path})

    async def copy(self, from_path: str, to_path: str) -> None:
        await self._request("POST", f"/sandboxes/{self.id}/copy", {"from": from_path, "to": to_path})

    async def symlink(self, target: str, link_path: str) -> None:
        await self._request("POST", f"/sandboxes/{self.id}/symlink", {"target": target, "link_path": link_path})

    async def readlink(self, path: str) -> str:
        response = await self._request("POST", f"/sandboxes/{self.id}/readlink", {"path": path})
        return response["target"]

    async def truncate(self, path: str, size: int) -> None:
        await self._request("POST", f"/sandboxes/{self.id}/truncate", {"path": path, "size": size})

    async def list_dir(self, path: str) -> list[DirEntry]:
        response = await self._request("POST", f"/sandboxes/{self.id}/list-dir", {"path": path})
        return [
            DirEntry(
                name=e["name"],
                is_dir=e["is_dir"],
                is_symlink=e["is_symlink"],
                size=e["size"],
                mode=e["mode"],
                mtime=datetime.fromtimestamp(e["mtime_unix"], tz=timezone.utc),
            )
            for e in response["entries"]
        ]

    async def mount(
        self,
        bucket: str,
        endpoint: str,
        access_key: str,
        secret_key: str,
        mount_path: str,
        read_only: bool = False,
    ) -> MountInfo:
        body = {
            "bucket": bucket,
            "endpoint": endpoint,
            "access_key": access_key,
            "secret_key": secret_key,
            "mount_path": mount_path,
            "read_only": read_only,
        }
        response = await self._request("POST", f"/sandboxes/{self.id}/mounts", body)
        return _mount_info_from_response(response)

    async def list_mounts(self) -> list[MountInfo]:
        response = await self._request("GET", f"/sandboxes/{self.id}/mounts")
        return [_mount_info_from_response(m) for m in response["mounts"]]

    async def unmount(self, mount_id: str) -> None:
        await self._request("DELETE", f"/sandboxes/{self.id}/mounts/{mount_id}")

    async def stop(self, keep: bool | None = None) -> StopResult:
        path = f"/sandboxes/{self.id}"
        if keep is not None:
            path += f"?{urlencode({'keep': 'true' if keep else 'false'})}"
        response = await self._request("DELETE", path)
        if response is None:
            return StopResult(kept=False, snapshot_id=None)
        return StopResult(kept=response["kept"], snapshot_id=response["snapshot_id"])

    def preview_url(self, port: int, path: str = "/") -> str:
        """Pure and network-free, like `Sandbox.preview_url` — not a
        coroutine, since there's nothing to await."""
        if isinstance(port, bool) or not isinstance(port, int) or port < 1 or port > 65535:
            raise ValueError(f"invalid preview port: {port}")
        normalized_path = path if path.startswith("/") else f"/{path}"
        suffix = f"?{urlencode({'token': self._auth_token})}" if self._auth_token else ""
        return f"{self._base_url}/sandboxes/{self.id}/preview/{port}{normalized_path}{suffix}"

    async def snapshot(self) -> str:
        response = await self._request("POST", f"/sandboxes/{self.id}/snapshot")
        return response["snapshot_id"]

    @classmethod
    async def resume(cls, snapshot_id: str, base_url: str | None = None, auth_token: str | None = None) -> "AsyncSandbox":
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = await request(resolved_base_url, "POST", f"/snapshots/{snapshot_id}/resume", resolved_token)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    async def fork(cls, snapshot_id: str, base_url: str | None = None, auth_token: str | None = None) -> "AsyncSandbox":
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = await request(resolved_base_url, "POST", f"/snapshots/{snapshot_id}/fork", resolved_token)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    async def list_snapshots(
        cls,
        source_sandbox_id: str | None = None,
        base_url: str | None = None,
        auth_token: str | None = None,
    ) -> list[SnapshotInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        query = urlencode({"source_sandbox_id": source_sandbox_id}) if source_sandbox_id else ""
        path = f"/snapshots?{query}" if query else "/snapshots"
        response = await request(resolved_base_url, "GET", path, resolved_token)
        return [
            SnapshotInfo(
                id=summary["id"],
                source_sandbox_id=summary["source_sandbox_id"],
                created_at=datetime.fromtimestamp(summary["created_at_unix"], tz=timezone.utc),
                tags=summary["tags"],
                forked_into=summary["forked_into"],
                name=summary.get("name"),
            )
            for summary in response["snapshots"]
        ]

    async def _request(self, method: str, path: str, body=None):
        return await request(self._base_url, method, path, self._auth_token, body)

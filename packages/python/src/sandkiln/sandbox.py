from __future__ import annotations

import base64
from dataclasses import dataclass, field
from datetime import datetime, timezone
from urllib.parse import urlencode

from ._config import resolve_auth_token, resolve_base_url
from ._http import request


@dataclass(frozen=True)
class DriveAttachment:
    """A persistent drive (see `Drive.create`) to attach at boot, each
    becoming its own block device inside the guest. A read-write
    attachment (the default, `read_only=False`) needs exclusive access -
    the daemon rejects it with a 409 if the drive is already attached
    anywhere; `read_only=True` may coexist with any number of other
    read-only attachments of the same drive, but still conflicts with an
    existing read-write one."""

    id: str
    read_only: bool = False


@dataclass(frozen=True)
class ExecResult:
    stdout: str
    stderr: str
    exit_code: int


@dataclass(frozen=True)
class DirEntry:
    """One entry from `Sandbox.list_dir()` - `mode` is permission bits
    only (e.g. `0o644`), the same shape `Sandbox.chmod()` takes, so an
    entry's `mode` can be handed straight back to `chmod()`."""

    name: str
    is_dir: bool
    is_symlink: bool
    size: int
    mode: int
    mtime: datetime


@dataclass(frozen=True)
class SandboxInfo:
    id: str
    created_at: datetime
    tags: dict[str, str] = field(default_factory=dict)
    name: str | None = None


@dataclass(frozen=True)
class StopResult:
    """Result of `Sandbox.stop()`. `kept` is `False` either because the
    caller explicitly asked for full destruction (`keep=False`) or because
    this particular sandbox had nothing new to preserve (a fork of a
    snapshot — its state already lives in the snapshot it came from)."""

    kept: bool
    snapshot_id: str | None = None


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
    # Carried over from the sandbox this was taken from, if any — see
    # `CreateSandboxOptions`'s `name`.
    name: str | None = None


def _drive_attachment_to_dict(attachment: DriveAttachment) -> dict[str, object]:
    return {"id": attachment.id, "read_only": attachment.read_only}


def _build_rate_limit(bandwidth_bytes_per_sec: int | None, ops_per_sec: int | None) -> dict[str, int] | None:
    """`None` when the caller didn't set either sub-field — only includes
    the ones actually set, mirroring the daemon's own per-field
    `#[serde(default)]` optionality rather than always sending both keys."""
    if bandwidth_bytes_per_sec is None and ops_per_sec is None:
        return None
    body: dict[str, int] = {}
    if bandwidth_bytes_per_sec is not None:
        body["bandwidth_bytes_per_sec"] = bandwidth_bytes_per_sec
    if ops_per_sec is not None:
        body["ops_per_sec"] = ops_per_sec
    return body


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
    ) -> "Sandbox":
        """`name` is a caller-given identity, unique among live sandboxes
        and held snapshots at the moment it's claimed — the daemon rejects
        a taken name with a 409. Optional; omit for an anonymous sandbox,
        same as before this existed. See `Sandbox.by_name`/
        `Sandbox.get_or_create` for finding a named sandbox again later.

        `vcpu_count`/`mem_size_mib` override the daemon's configured
        defaults for this one sandbox; omitted (the default) uses them
        unchanged. The daemon rejects a value of `0` or anything above its
        configured ceiling (`SANDKILN_MAX_VCPU_COUNT`/
        `SANDKILN_MAX_MEM_SIZE_MIB`) with a 400.

        `image_id` boots from a registered image (see `Image.register`)
        instead of the daemon's configured default rootfs; omitted keeps
        today's behavior unchanged. Raises `SandkilnApiError` with status
        404 if no image with this id is currently registered.

        `rate_limit_bandwidth_bytes_per_sec`/`rate_limit_ops_per_sec` cap
        host I/O via Firecracker's own token-bucket rate limiter, applied
        to the rootfs drive, every attached drive, and both directions of
        the network interface. Both omitted (the default) means unlimited
        host I/O, unchanged from before this existed. A `0` for either
        raises `SandkilnApiError` with status 400 — meaningless, rejected
        outright rather than silently treated as unlimited.

        `drives` attaches existing persistent drives (see `Drive.create`)
        at boot time. Omitted means no drives, unchanged from before this
        existed."""
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
        response = request(resolved_base_url, "POST", "/sandboxes", resolved_token, body or None)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    def attach(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> "Sandbox":
        """Wraps an already-existing sandbox id without a network
        round-trip — for a process that only has an id from elsewhere and
        needs a handle to call instance methods on."""
        return cls(id, resolve_base_url(base_url), resolve_auth_token(auth_token))

    @classmethod
    def by_name(cls, name: str, base_url: str | None = None, auth_token: str | None = None) -> "Sandbox":
        """Resolves a name to a live sandbox and returns a handle to it —
        a network round-trip, unlike `attach`, since the id isn't known up
        front. Only resolves a *live* sandbox: if this name currently
        belongs to a stopped (snapshotted) sandbox instead, the daemon
        raises `SandkilnApiError` with status 409 rather than silently
        resuming it — use `Sandbox.get_or_create` if that's what you
        want."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = request(resolved_base_url, "GET", f"/sandboxes/by-name/{name}", resolved_token)
        return cls(response["id"], resolved_base_url, resolved_token)

    @classmethod
    def get_or_create(
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
    ) -> tuple["Sandbox", bool]:
        """Resolves `name` to a sandbox in one call, creating it if it
        doesn't exist yet: a live sandbox with this name is returned
        as-is, a stopped (snapshotted) one is resumed, and otherwise a
        fresh sandbox is created and given this name. `tags`/
        `vcpu_count`/`mem_size_mib`/rate limit options only apply to the
        create-fresh case — resuming an existing snapshot uses what was
        recorded on it when it was taken, same as `Sandbox.resume`. See
        `Sandbox.create` for what the rate-limit options mean.

        Returns `(sandbox, created)` — `created` is `True` only when this
        call actually booted a brand-new sandbox from the base rootfs.

        Race-safe on the daemon side: two concurrent calls for the same
        brand-new name can't both create a sandbox — the second sees the
        first's result instead."""
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
        response = request(resolved_base_url, "POST", "/sandboxes/get-or-create", resolved_token, body)
        return cls(response["id"], resolved_base_url, resolved_token), response["created"]

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
                name=summary.get("name"),
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

    def chmod(self, path: str, mode: int) -> None:
        """`mode` is raw permission bits (e.g. `0o644`), the same shape
        POSIX `chmod(2)` takes - not a symbolic string like the `chmod`
        shell command accepts."""
        self._request("POST", f"/sandboxes/{self.id}/chmod", {"path": path, "mode": mode})

    def chown(self, path: str, uid: int, gid: int) -> None:
        self._request("POST", f"/sandboxes/{self.id}/chown", {"path": path, "uid": uid, "gid": gid})

    def mkdir(self, path: str, parents: bool = False) -> None:
        """`parents=True` behaves like `mkdir -p` (creates missing parent
        directories, succeeds if the target already exists); `False`
        (the default) behaves like plain `mkdir` - fails if the parent is
        missing or the target already exists."""
        self._request("POST", f"/sandboxes/{self.id}/mkdir", {"path": path, "parents": parents})

    def rename(self, from_path: str, to_path: str) -> None:
        self._request("POST", f"/sandboxes/{self.id}/rename", {"from": from_path, "to": to_path})

    def copy(self, from_path: str, to_path: str) -> None:
        """A full byte-for-byte copy to a new path - `from_path` is left
        untouched, unlike `rename`."""
        self._request("POST", f"/sandboxes/{self.id}/copy", {"from": from_path, "to": to_path})

    def symlink(self, target: str, link_path: str) -> None:
        self._request("POST", f"/sandboxes/{self.id}/symlink", {"target": target, "link_path": link_path})

    def readlink(self, path: str) -> str:
        """The target a symlink points at, exactly as stored - not
        resolved/canonicalized."""
        response = self._request("POST", f"/sandboxes/{self.id}/readlink", {"path": path})
        return response["target"]

    def truncate(self, path: str, size: int) -> None:
        self._request("POST", f"/sandboxes/{self.id}/truncate", {"path": path, "size": size})

    def list_dir(self, path: str) -> list[DirEntry]:
        response = self._request("POST", f"/sandboxes/{self.id}/list-dir", {"path": path})
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

    def stop(self, keep: bool | None = None) -> StopResult:
        """Stops this sandbox. By default (`keep` omitted, or `True`) this
        *preserves* its state: internally the daemon does what
        `Sandbox.snapshot()` does (pause, snapshot to disk, stop the VM)
        and reports the resulting snapshot id — "stop and come back
        later" is the default, not something you have to manage yourself.
        Resume it with `Sandbox.resume(snapshot_id)`, or find it again by
        name with `Sandbox.get_or_create(name=...)` if this sandbox had
        one.

        Pass `keep=False` for the old "just destroy it" behavior — no
        snapshot, nothing left behind — for a sandbox you genuinely never
        want back (e.g. a short-lived CI run)."""
        path = f"/sandboxes/{self.id}"
        if keep is not None:
            path += f"?{urlencode({'keep': 'true' if keep else 'false'})}"
        # `keep=False` gets a bare 204 back (no body) — `_http.request`
        # decodes that as `None`. Normalized here so callers get one
        # consistent return type regardless of which path the daemon took.
        response = self._request("DELETE", path)
        if response is None:
            return StopResult(kept=False, snapshot_id=None)
        return StopResult(kept=response["kept"], snapshot_id=response["snapshot_id"])

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
                name=summary.get("name"),
            )
            for summary in response["snapshots"]
        ]

    def _request(self, method: str, path: str, body=None):
        return request(self._base_url, method, path, self._auth_token, body)

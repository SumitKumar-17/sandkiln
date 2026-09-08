from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime, timezone

from ._config import resolve_auth_token, resolve_base_url
from ._http import request


@dataclass(frozen=True)
class DriveHolder:
    """Who currently holds a drive - `"sandbox <id>"` or
    `"snapshot <id>"`. More than one holder means it's attached read-only
    to several at once; a drive can't be deleted while any exist."""

    holder: str
    read_only: bool


@dataclass(frozen=True)
class DriveInfo:
    id: str
    size_mib: int
    created_at: datetime
    attached_to: list[DriveHolder] = field(default_factory=list)


class Drive:
    """Persistent drives: attachable filesystem storage that outlives any
    single sandbox and can be reattached to a new one - for state that
    should survive well past any one VM's lifetime. Attach one at create
    time via `Sandbox.create`'s `drives` argument. Like `Image`, a
    namespace of classmethods rather than a stateful handle - a drive has
    no instance behavior besides delete.

    A drive has no "detach" call of its own: detaching happens implicitly
    when the sandbox holding it is stopped (`Sandbox.stop`), which drops
    the attachment without touching the drive's backing file.
    `Drive.delete` permanently destroys a drive and raises (409) while
    anything still holds it.
    """

    @classmethod
    def create(cls, size_mib: int, base_url: str | None = None, auth_token: str | None = None) -> DriveInfo:
        """Creates a new empty drive of `size_mib` MiB, ready to attach to
        a sandbox at create time."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = request(resolved_base_url, "POST", "/drives", resolved_token, {"size_mib": size_mib})
        return _to_drive_info(response)

    @classmethod
    def list(cls, base_url: str | None = None, auth_token: str | None = None) -> list[DriveInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = request(resolved_base_url, "GET", "/drives", resolved_token)
        return [_to_drive_info(summary) for summary in response["drives"]]

    @classmethod
    def delete(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> None:
        """Permanently removes a drive and its backing file. Raises
        `SandkilnApiError` with status 409 while any sandbox or held
        snapshot still attaches it - see `DriveInfo.attached_to`."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        request(resolved_base_url, "DELETE", f"/drives/{id}", resolved_token)


def _to_drive_info(summary: dict) -> DriveInfo:
    return DriveInfo(
        id=summary["id"],
        size_mib=summary["size_mib"],
        created_at=datetime.fromtimestamp(summary["created_at_unix"], tz=timezone.utc),
        attached_to=[DriveHolder(holder=h["holder"], read_only=h["read_only"]) for h in summary["attached_to"]],
    )

"""`AsyncDrive`: an `asyncio`-native mirror of `Drive` — see `asandbox.py`'s
module doc comment for why this is a separate class rather than one
class with both sync and async methods."""

from __future__ import annotations

from ._config import resolve_auth_token, resolve_base_url
from ._http_async import request
from .drive import DriveInfo, _to_drive_info


class AsyncDrive:
    """See `Drive` (the sync equivalent) for full documentation — this
    class is identical in behavior, only the calling convention
    (`await`) differs. A namespace of classmethods, same as `Drive`."""

    @classmethod
    async def create(cls, size_mib: int, base_url: str | None = None, auth_token: str | None = None) -> DriveInfo:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = await request(resolved_base_url, "POST", "/drives", resolved_token, {"size_mib": size_mib})
        return _to_drive_info(response)

    @classmethod
    async def list(cls, base_url: str | None = None, auth_token: str | None = None) -> list[DriveInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = await request(resolved_base_url, "GET", "/drives", resolved_token)
        return [_to_drive_info(summary) for summary in response["drives"]]

    @classmethod
    async def delete(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> None:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        await request(resolved_base_url, "DELETE", f"/drives/{id}", resolved_token)

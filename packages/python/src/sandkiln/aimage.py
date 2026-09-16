"""`AsyncImage`: an `asyncio`-native mirror of `Image` — see
`asandbox.py`'s module doc comment for why this is a separate class
rather than one class with both sync and async methods."""

from __future__ import annotations

from ._config import resolve_auth_token, resolve_base_url
from ._http_async import request
from .image import ImageInfo, _to_image_info


class AsyncImage:
    """See `Image` (the sync equivalent) for full documentation — this
    class is identical in behavior, only the calling convention
    (`await`) differs. A namespace of classmethods, same as `Image`."""

    @classmethod
    async def register(
        cls,
        id: str,
        path: str,
        base_url: str | None = None,
        auth_token: str | None = None,
    ) -> ImageInfo:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = await request(resolved_base_url, "POST", "/images", resolved_token, {"id": id, "path": path})
        return _to_image_info(response)

    @classmethod
    async def list(cls, base_url: str | None = None, auth_token: str | None = None) -> list[ImageInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = await request(resolved_base_url, "GET", "/images", resolved_token)
        return [_to_image_info(summary) for summary in response["images"]]

    @classmethod
    async def delete(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> None:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        await request(resolved_base_url, "DELETE", f"/images/{id}", resolved_token)

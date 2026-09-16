"""`AsyncPool`: an `asyncio`-native mirror of `Pool` — see `asandbox.py`'s
module doc comment for why this is a separate class rather than one
class with both sync and async methods."""

from __future__ import annotations

from ._config import resolve_auth_token, resolve_base_url
from ._http_async import request
from .pool import PoolInfo, _to_pool_info


class AsyncPool:
    """See `Pool` (the sync equivalent) for full documentation — this
    class is identical in behavior, only the calling convention
    (`await`) differs. A namespace of classmethods, same as `Pool`."""

    @classmethod
    async def create(
        cls,
        id: str,
        image_id: str | None = None,
        vcpu_count: int | None = None,
        mem_size_mib: int | None = None,
        warm_count: int = 0,
        max_count: int | None = None,
        base_url: str | None = None,
        auth_token: str | None = None,
    ) -> PoolInfo:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        body: dict[str, object] = {"id": id, "warm_count": warm_count}
        if image_id is not None:
            body["image_id"] = image_id
        if vcpu_count is not None:
            body["vcpu_count"] = vcpu_count
        if mem_size_mib is not None:
            body["mem_size_mib"] = mem_size_mib
        if max_count is not None:
            body["max_count"] = max_count
        response = await request(resolved_base_url, "POST", "/pools", resolved_token, body)
        return _to_pool_info(response)

    @classmethod
    async def list(cls, base_url: str | None = None, auth_token: str | None = None) -> list[PoolInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = await request(resolved_base_url, "GET", "/pools", resolved_token)
        return [_to_pool_info(summary) for summary in response["pools"]]

    @classmethod
    async def delete(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> None:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        await request(resolved_base_url, "DELETE", f"/pools/{id}", resolved_token)

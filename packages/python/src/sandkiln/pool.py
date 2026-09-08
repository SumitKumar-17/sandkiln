from __future__ import annotations

from dataclasses import dataclass

from ._config import resolve_auth_token, resolve_base_url
from ._http import request


@dataclass(frozen=True)
class PoolInfo:
    """A configured pre-warmed pool - see `Pool`. `vcpu_count`/
    `mem_size_mib` are already resolved to concrete values (whatever the
    daemon's own defaults were at the moment this pool was created), not
    the `None`-means-"use the default" shape `Pool.create`'s own
    arguments take."""

    id: str
    image_id: str | None
    vcpu_count: int
    mem_size_mib: int
    warm_count: int
    warm_ready: int
    """How many resumable snapshots are actually sitting warm right now -
    can be less than `warm_count` right after the pool is created or a
    claim just drained it; replenishment happens in the background, not
    instantly."""


class Pool:
    """Pre-warmed pools: keep a small number of ready-to-resume snapshots
    around for a given image/resource config, so a matching
    `Sandbox.create()` can resume one instead of paying full cold-create
    cost. Like `Image`/`Drive`, a namespace of classmethods rather than a
    stateful handle - a pool has no instance behavior besides delete.

    Claiming from a pool is entirely transparent: `Sandbox.create()`
    itself matches a plain create (no `drives`, no `rate_limit`) against
    every configured pool by `image_id`/`vcpu_count`/`mem_size_mib` and
    resumes a warm snapshot automatically when one is ready - there's no
    separate "create from pool" call. See ROADMAP.md's "Persistence and
    snapshotting" section (Pre-warmed snapshot pool) in the sandkiln
    repository for the full design, including a real, non-rare failure
    mode a claim's own post-resume health check protects against.
    """

    @classmethod
    def create(
        cls,
        id: str,
        image_id: str | None = None,
        vcpu_count: int | None = None,
        mem_size_mib: int | None = None,
        warm_count: int = 0,
        base_url: str | None = None,
        auth_token: str | None = None,
    ) -> PoolInfo:
        """Configures a new pool under `id` - a stable name used to refer
        back to this configuration (not exposed to the guest or any
        sandbox created from it). Raises `SandkilnApiError` with status
        409 if `id` is already taken; delete it first to reconfigure.
        `warm_count` is how many resumable snapshots to keep ready at
        once - replenishment happens in the background and isn't
        instant, see `PoolInfo.warm_ready`."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        body: dict[str, object] = {"id": id, "warm_count": warm_count}
        if image_id is not None:
            body["image_id"] = image_id
        if vcpu_count is not None:
            body["vcpu_count"] = vcpu_count
        if mem_size_mib is not None:
            body["mem_size_mib"] = mem_size_mib
        response = request(resolved_base_url, "POST", "/pools", resolved_token, body)
        return _to_pool_info(response)

    @classmethod
    def list(cls, base_url: str | None = None, auth_token: str | None = None) -> list[PoolInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = request(resolved_base_url, "GET", "/pools", resolved_token)
        return [_to_pool_info(summary) for summary in response["pools"]]

    @classmethod
    def delete(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> None:
        """Removes a pool's configuration and destroys whatever it
        currently has warm. A sandbox already claimed from this pool is
        unaffected - only the pool's own not-yet-claimed warm snapshots
        are cleaned up."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        request(resolved_base_url, "DELETE", f"/pools/{id}", resolved_token)


def _to_pool_info(summary: dict) -> PoolInfo:
    return PoolInfo(
        id=summary["id"],
        image_id=summary["image_id"],
        vcpu_count=summary["vcpu_count"],
        mem_size_mib=summary["mem_size_mib"],
        warm_count=summary["warm_count"],
        warm_ready=summary["warm_ready"],
    )

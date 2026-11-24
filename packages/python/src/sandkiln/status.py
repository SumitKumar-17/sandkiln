from __future__ import annotations

from dataclasses import dataclass

from ._config import resolve_auth_token, resolve_base_url
from ._http import request


@dataclass(frozen=True)
class DaemonStatus:
    """Host health and capacity, from `GET /status`. `warnings` holds
    plain-language conditions to act on and is empty when healthy."""

    version: str
    uptime_secs: int
    auth_enabled: bool
    jailer_enabled: bool
    sandboxes: int
    held_snapshots: int
    retired_checkpoints: int
    pools: int
    pool_warm_ready: int
    tap_devices_free: int
    tap_devices_total: int
    sandboxes_created_total: int
    rootfs_clones_reflink: int
    rootfs_clones_copy: int
    disk: list[dict]
    warnings: list[str]


def get_status(base_url: str | None = None, auth_token: str | None = None) -> DaemonStatus:
    """Fetch the daemon's `/status` document."""
    body = request(resolve_base_url(base_url), "GET", "/status", resolve_auth_token(auth_token))
    return to_daemon_status(body)


def to_daemon_status(body: dict) -> DaemonStatus:
    counts = body["counts"]
    return DaemonStatus(
        version=body["version"],
        uptime_secs=body["uptime_secs"],
        auth_enabled=body["auth_enabled"],
        jailer_enabled=body["jailer_enabled"],
        sandboxes=counts["sandboxes"],
        held_snapshots=counts["held_snapshots"],
        retired_checkpoints=counts["retired_checkpoints"],
        pools=counts["pools"],
        pool_warm_ready=counts["pool_warm_ready"],
        tap_devices_free=body["tap_pool"]["free"],
        tap_devices_total=body["tap_pool"]["size"],
        sandboxes_created_total=body["sandboxes_created_total"],
        rootfs_clones_reflink=body["rootfs_clones"]["reflink"],
        rootfs_clones_copy=body["rootfs_clones"]["copy"],
        disk=body["disk"],
        warnings=body["warnings"],
    )

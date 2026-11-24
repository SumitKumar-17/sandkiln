from __future__ import annotations

from ._config import resolve_auth_token, resolve_base_url
from ._http_async import request
from .status import DaemonStatus, to_daemon_status


async def get_status(base_url: str | None = None, auth_token: str | None = None) -> DaemonStatus:
    """`asyncio` mirror of `sandkiln.status.get_status`."""
    body = await request(resolve_base_url(base_url), "GET", "/status", resolve_auth_token(auth_token))
    return to_daemon_status(body)

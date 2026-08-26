import os

_DEFAULT_BASE_URL = "http://127.0.0.1:7777"


def resolve_base_url(base_url: str | None = None) -> str:
    chosen = base_url or os.environ.get("SANDKILN_DAEMON_URL") or _DEFAULT_BASE_URL
    return chosen.rstrip("/")


def resolve_auth_token(auth_token: str | None = None) -> str | None:
    return auth_token or os.environ.get("SANDKILN_AUTH_TOKEN")

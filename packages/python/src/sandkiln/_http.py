import json
import urllib.error
import urllib.request
from typing import Any

from .errors import SandkilnApiError


def request(
    base_url: str,
    method: str,
    path: str,
    auth_token: str | None = None,
    body: Any = None,
) -> Any:
    headers: dict[str, str] = {}
    data: bytes | None = None
    if body is not None:
        data = json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    if auth_token is not None:
        headers["Authorization"] = f"Bearer {auth_token}"

    req = urllib.request.Request(f"{base_url}{path}", data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req) as response:
            return _parse_body(response)
    except urllib.error.HTTPError as error:
        raise SandkilnApiError(error.code, _extract_error_message(error)) from None
    except urllib.error.URLError as error:
        raise SandkilnApiError(0, f"could not reach {base_url}: {error.reason}") from None


def _parse_body(response: Any) -> Any:
    raw = response.read()
    if not raw:
        return None
    return json.loads(raw)


def _extract_error_message(error: urllib.error.HTTPError) -> str:
    try:
        body = json.loads(error.read())
        if isinstance(body, dict) and isinstance(body.get("error"), str):
            return body["error"]
    except (json.JSONDecodeError, UnicodeDecodeError):
        pass
    return error.reason or f"request failed with status {error.code}"

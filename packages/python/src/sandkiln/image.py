from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone

from ._config import resolve_auth_token, resolve_base_url
from ._http import request


@dataclass(frozen=True)
class ImageInfo:
    id: str
    size_mib: int
    created_at: datetime
    in_use_by: str | None
    guest_agent_verified: bool
    verification_hint: str


class Image:
    """Registered rootfs images: named, daemon-tracked ext4 files a
    sandbox can boot from instead of the daemon's configured default
    rootfs (see `Sandbox.create`'s `image_id`). A namespace of
    classmethods rather than a stateful handle like `Sandbox` - an image
    has no instance behavior besides delete, and delete only ever needs
    an id.

    Registration does not upload a file: `path` must already exist on the
    host the daemon process itself runs on. Converting a Docker/OCI image
    into a bootable rootfs is a separate, larger problem this SDK (and
    the daemon) doesn't attempt - see images/README.md in the sandkiln
    repository.
    """

    @classmethod
    def register(
        cls,
        id: str,
        path: str,
        base_url: str | None = None,
        auth_token: str | None = None,
    ) -> ImageInfo:
        """Registers an already-built ext4 rootfs file at `path` (a path
        on the daemon's own host filesystem, not uploaded) under `id` - a
        stable, memorable name reusable across many
        `Sandbox.create(image_id=...)` calls.

        The daemon cannot verify the guest agent is baked into the image:
        that would need loop-mounting the file as root, which the daemon
        deliberately doesn't have (see `ImageInfo.guest_agent_verified`).
        Check `verification_hint` on the result, or run
        `scripts/preflight-check.sh --root-checks --rootfs-image <path>`
        against the file out of band before relying on it - the single
        most common way a custom image otherwise fails is booting fine
        but never responding to `run_command`.
        """
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = request(resolved_base_url, "POST", "/images", resolved_token, {"id": id, "path": path})
        return _to_image_info(response)

    @classmethod
    def list(cls, base_url: str | None = None, auth_token: str | None = None) -> list[ImageInfo]:
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        response = request(resolved_base_url, "GET", "/images", resolved_token)
        return [_to_image_info(summary) for summary in response["images"]]

    @classmethod
    def delete(cls, id: str, base_url: str | None = None, auth_token: str | None = None) -> None:
        """Permanently removes an image's registration and backing file.
        Raises `SandkilnApiError` with status 409 while any live sandbox,
        in-flight boot, or held snapshot still references it."""
        resolved_base_url = resolve_base_url(base_url)
        resolved_token = resolve_auth_token(auth_token)
        request(resolved_base_url, "DELETE", f"/images/{id}", resolved_token)


def _to_image_info(summary: dict) -> ImageInfo:
    return ImageInfo(
        id=summary["id"],
        size_mib=summary["size_mib"],
        created_at=datetime.fromtimestamp(summary["created_at_unix"], tz=timezone.utc),
        in_use_by=summary["in_use_by"],
        guest_agent_verified=summary["guest_agent_verified"],
        verification_hint=summary["verification_hint"],
    )

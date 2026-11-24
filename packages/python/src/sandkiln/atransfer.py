"""`asyncio` mirror of `transfer.py`'s `upload_dir`/`download_dir`: same
rules, arguments and result, with sandbox calls awaited and local file I/O
moved to a worker thread so it cannot stall the event loop. Every pure
helper and `TransferResult` is the sync module's."""

from __future__ import annotations

import asyncio
import os
from typing import Any

from .transfer import (
    DEFAULT_CONCURRENCY,
    Exclude,
    TransferResult,
    check_unique_names,
    make_local_dir,
    posix_join,
    relative_path,
    safe_child,
    scan_local,
    symlink_stays_inside,
    validate_concurrency,
    write_local_file,
)


def _read_local(path: str) -> tuple[bytes, int]:
    with open(path, "rb") as handle:
        content = handle.read()
    return content, os.lstat(path).st_mode & 0o7777


async def _run_limited(tasks: list[Any], concurrency: int) -> list[Any]:
    gate = asyncio.Semaphore(concurrency)

    async def run(task: Any) -> Any:
        async with gate:
            return await task

    return await asyncio.gather(*(run(t) for t in tasks))


async def upload_dir(
    target: Any,
    local_dir: str,
    remote_dir: str,
    exclude: Exclude | None = None,
    concurrency: int = DEFAULT_CONCURRENCY,
) -> TransferResult:
    validate_concurrency(concurrency)
    result = TransferResult()
    files: list[tuple[str, str]] = []
    links: list[tuple[str, str]] = []

    await target.mkdir(remote_dir, parents=True)
    result.directories += 1

    async def walk(local: str, remote: str, relative: str) -> None:
        for entry in await asyncio.to_thread(scan_local, local):
            rel = relative_path(relative, entry.name)
            if exclude is not None and exclude(rel):
                result.skipped.append(rel)
                continue
            remote_path = posix_join(remote, entry.name)
            if entry.kind == "dir":
                await target.mkdir(remote_path, parents=True)
                result.directories += 1
                await walk(entry.path, remote_path, rel)
            elif entry.kind == "symlink":
                links.append((os.readlink(entry.path), remote_path))
            elif entry.kind == "file":
                files.append((entry.path, remote_path))
            else:
                result.skipped.append(rel)

    await walk(local_dir, remote_dir, "")

    async def send(local_path: str, remote_path: str) -> int:
        content, mode = await asyncio.to_thread(_read_local, local_path)
        await target.write_file(remote_path, content)
        await target.chmod(remote_path, mode)
        return len(content)

    for size in await _run_limited([send(lp, rp) for lp, rp in files], concurrency):
        result.files += 1
        result.bytes += size
    for link_target, link_path in links:
        await target.symlink(link_target, link_path)
        result.symlinks += 1
    return result


async def download_dir(
    target: Any,
    remote_dir: str,
    local_dir: str,
    exclude: Exclude | None = None,
    concurrency: int = DEFAULT_CONCURRENCY,
) -> TransferResult:
    validate_concurrency(concurrency)
    result = TransferResult()
    files: list[tuple[str, str, int]] = []
    root = os.path.realpath(local_dir)
    os.makedirs(root, exist_ok=True)
    result.directories += 1

    async def walk(remote: str, local: str, relative: str) -> None:
        entries = await target.list_dir(remote)
        check_unique_names(e.name for e in entries)
        for entry in entries:
            local_path = safe_child(local, entry.name)
            rel = relative_path(relative, entry.name)
            if exclude is not None and exclude(rel):
                result.skipped.append(rel)
                continue
            remote_path = posix_join(remote, entry.name)
            if entry.is_symlink:
                link_target = await target.readlink(remote_path)
                if symlink_stays_inside(root, local, link_target):
                    os.symlink(link_target, local_path)
                    result.symlinks += 1
                else:
                    result.skipped.append(rel)
            elif entry.is_dir:
                make_local_dir(local_path)
                result.directories += 1
                await walk(remote_path, local_path, rel)
            else:
                files.append((remote_path, local_path, entry.mode))

    await walk(remote_dir, root, "")

    async def fetch(remote_path: str, local_path: str, mode: int) -> int:
        content = await target.read_file(remote_path)
        await asyncio.to_thread(write_local_file, local_path, content, mode)
        return len(content)

    for size in await _run_limited([fetch(*f) for f in files], concurrency):
        result.files += 1
        result.bytes += size
    return result

"""Directory transfer for `Sandbox.upload_dir()` / `download_dir()`
(`atransfer.py` is the `asyncio` mirror). Mirrors the JS SDK's
`transfer.ts`, with the same safety rules for the one direction that
handles untrusted input: a directory listing from the guest.

- Entry names must be a single plain path component (`safe_child`), so a
  hostile guest cannot write outside the destination with `..` or a path
  separator in a name.
- A symlink is only recreated when its target stays inside the destination
  (`symlink_stays_inside`); others are reported in `skipped`, not written.
  The check resolves symlinks already on disk, which the JS check does not:
  a lexical test is fooled by `a -> ..` followed by `b -> a/..`.
- Two entries with one name in a listing are refused, and files are opened
  with `O_NOFOLLOW`, so an entry cannot be written through a link made by
  an earlier one.
"""

from __future__ import annotations

import os
import stat
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Callable, Iterable, Protocol

DEFAULT_CONCURRENCY = 8


@dataclass
class TransferResult:
    """What an upload or download did. `skipped` holds entries left out:
    excluded ones, special files, and symlinks that would escape the
    destination (relative paths, `/`-separated)."""

    files: int = 0
    directories: int = 0
    symlinks: int = 0
    bytes: int = 0
    skipped: list[str] = field(default_factory=list)


class TransferTarget(Protocol):
    """The slice of `Sandbox` the helpers use, so tests can pass a fake."""

    def read_file(self, path: str) -> bytes: ...
    def write_file(self, path: str, content: str | bytes) -> None: ...
    def mkdir(self, path: str, parents: bool = False) -> None: ...
    def chmod(self, path: str, mode: int) -> None: ...
    def symlink(self, target: str, link_path: str) -> None: ...
    def readlink(self, path: str) -> str: ...
    def list_dir(self, path: str): ...


Exclude = Callable[[str], bool]


def validate_concurrency(concurrency: int) -> int:
    if isinstance(concurrency, bool) or not isinstance(concurrency, int) or concurrency < 1:
        raise ValueError(f"concurrency must be a positive integer, got {concurrency!r}")
    return concurrency


def safe_child(root: str, name: str) -> str:
    """`name` joined under `root`, refusing anything that is not a single
    plain path component. Keeps a hostile listing from escaping `root`."""
    if name in ("", ".", "..") or "/" in name or "\\" in name or "\0" in name:
        raise ValueError(f"refusing unsafe entry name {name!r}")
    return os.path.join(root, name)


def symlink_stays_inside(root: str, link_dir: str, target: str) -> bool:
    """True if a symlink created in `link_dir` pointing at `target` resolves
    inside `root`. Absolute targets never do. Symlinks already on disk along
    the way are followed, so a chain of individually harmless links cannot
    add up to an escape."""
    if os.path.isabs(target) or "\0" in target:
        return False
    real_root = os.path.realpath(root)
    resolved = os.path.realpath(os.path.join(link_dir, target))
    try:
        return os.path.commonpath([real_root, resolved]) == real_root
    except ValueError:
        return False


def check_unique_names(names: Iterable[str]) -> None:
    """A listing never legitimately repeats a name; a hostile one might, to
    have a later entry written through an earlier entry's symlink."""
    seen: set[str] = set()
    for name in names:
        if name in seen:
            raise ValueError(f"refusing duplicate entry name {name!r} in one directory listing")
        seen.add(name)


def posix_join(directory: str, name: str) -> str:
    return directory + name if directory.endswith("/") else f"{directory}/{name}"


def relative_path(parent: str, name: str) -> str:
    return name if parent == "" else f"{parent}/{name}"


def write_local_file(path: str, content: bytes, mode: int) -> None:
    """Creates `path` private (0600), never following a symlink at `path`,
    then applies `mode` (permission bits only: setuid/setgid/sticky from an
    untrusted source are dropped)."""
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC | getattr(os, "O_NOFOLLOW", 0)
    with os.fdopen(os.open(path, flags, 0o600), "wb") as handle:
        handle.write(content)
        os.fchmod(handle.fileno(), mode & 0o777)


def make_local_dir(path: str) -> None:
    """Creates `path`, tolerating an existing real directory but never
    descending through a symlink that is already there."""
    if os.path.islink(path):
        raise ValueError(f"refusing to write through the existing symlink {path!r}")
    os.makedirs(path, exist_ok=True)


class LocalEntry:
    """One local directory entry for an upload, classified once."""

    __slots__ = ("name", "path", "kind")

    def __init__(self, name: str, path: str, kind: str) -> None:
        self.name = name
        self.path = path
        self.kind = kind  # "dir" | "symlink" | "file" | "other"


def scan_local(directory: str) -> list[LocalEntry]:
    entries: list[LocalEntry] = []
    with os.scandir(directory) as it:
        for entry in it:
            if entry.is_symlink():
                kind = "symlink"
            elif entry.is_dir(follow_symlinks=False):
                kind = "dir"
            elif stat.S_ISREG(entry.stat(follow_symlinks=False).st_mode):
                kind = "file"
            else:
                kind = "other"
            entries.append(LocalEntry(entry.name, entry.path, kind))
    entries.sort(key=lambda e: e.name)
    return entries


def upload_dir(
    target: TransferTarget,
    local_dir: str,
    remote_dir: str,
    exclude: Exclude | None = None,
    concurrency: int = DEFAULT_CONCURRENCY,
) -> TransferResult:
    """Copies a local directory tree into the sandbox, keeping file modes
    and symlinks. Files go over `concurrency` threads at a time."""
    validate_concurrency(concurrency)
    result = TransferResult()
    files: list[tuple[str, str]] = []
    links: list[tuple[str, str]] = []

    target.mkdir(remote_dir, parents=True)
    result.directories += 1

    def walk(local: str, remote: str, relative: str) -> None:
        for entry in scan_local(local):
            rel = relative_path(relative, entry.name)
            if exclude is not None and exclude(rel):
                result.skipped.append(rel)
                continue
            remote_path = posix_join(remote, entry.name)
            if entry.kind == "dir":
                target.mkdir(remote_path, parents=True)
                result.directories += 1
                walk(entry.path, remote_path, rel)
            elif entry.kind == "symlink":
                links.append((os.readlink(entry.path), remote_path))
            elif entry.kind == "file":
                files.append((entry.path, remote_path))
            else:
                result.skipped.append(rel)

    walk(local_dir, remote_dir, "")

    def send(item: tuple[str, str]) -> int:
        local_path, remote_path = item
        with open(local_path, "rb") as handle:
            content = handle.read()
        target.write_file(remote_path, content)
        target.chmod(remote_path, os.lstat(local_path).st_mode & 0o7777)
        return len(content)

    with ThreadPoolExecutor(max_workers=concurrency) as pool:
        for size in pool.map(send, files):
            result.files += 1
            result.bytes += size
    for link_target, link_path in links:
        target.symlink(link_target, link_path)
        result.symlinks += 1
    return result


def download_dir(
    target: TransferTarget,
    remote_dir: str,
    local_dir: str,
    exclude: Exclude | None = None,
    concurrency: int = DEFAULT_CONCURRENCY,
) -> TransferResult:
    """Copies a directory tree out of the sandbox. The guest is untrusted:
    entry names are validated (`safe_child`), duplicates refused, and a
    symlink is only recreated when its target stays inside `local_dir`;
    others are reported in `skipped`."""
    validate_concurrency(concurrency)
    result = TransferResult()
    files: list[tuple[str, str, int]] = []
    root = os.path.realpath(local_dir)
    os.makedirs(root, exist_ok=True)
    result.directories += 1

    def walk(remote: str, local: str, relative: str) -> None:
        entries = target.list_dir(remote)
        check_unique_names(e.name for e in entries)
        for entry in entries:
            local_path = safe_child(local, entry.name)
            rel = relative_path(relative, entry.name)
            if exclude is not None and exclude(rel):
                result.skipped.append(rel)
                continue
            remote_path = posix_join(remote, entry.name)
            if entry.is_symlink:
                link_target = target.readlink(remote_path)
                if symlink_stays_inside(root, local, link_target):
                    os.symlink(link_target, local_path)
                    result.symlinks += 1
                else:
                    result.skipped.append(rel)
            elif entry.is_dir:
                make_local_dir(local_path)
                result.directories += 1
                walk(remote_path, local_path, rel)
            else:
                files.append((remote_path, local_path, entry.mode))

    walk(remote_dir, root, "")

    def fetch(item: tuple[str, str, int]) -> int:
        remote_path, local_path, mode = item
        content = target.read_file(remote_path)
        write_local_file(local_path, content, mode)
        return len(content)

    with ThreadPoolExecutor(max_workers=concurrency) as pool:
        for size in pool.map(fetch, files):
            result.files += 1
            result.bytes += size
    return result

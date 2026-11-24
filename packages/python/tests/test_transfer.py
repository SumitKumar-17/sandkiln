"""Tests for `upload_dir`/`download_dir` (`transfer.py`, `atransfer.py`):
the path-safety helpers, and full trees against an in-memory fake sandbox,
including a hostile guest listing. Plain `test_*` functions; run as
`python tests/test_transfer.py` or under pytest.
"""

from __future__ import annotations

import asyncio
import os
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "src"))

from sandkiln import DirEntry  # noqa: E402
from sandkiln import atransfer  # noqa: E402
from sandkiln.transfer import (  # noqa: E402
    check_unique_names,
    download_dir,
    posix_join,
    safe_child,
    symlink_stays_inside,
    upload_dir,
    write_local_file,
)


def raises(exc, fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except exc as error:
        return error
    raise AssertionError(f"{exc.__name__} not raised")


class FakeSandbox:
    """An in-memory guest: `files` maps path to bytes, `dirs` is a set, `links` maps path to target,
    `modes` maps path to permission bits. `listings` overrides what `list_dir` returns (a hostile guest)."""

    def __init__(self, listings=None):
        self.files: dict[str, bytes] = {}
        self.dirs: set[str] = set()
        self.links: dict[str, str] = {}
        self.modes: dict[str, int] = {}
        self.listings = listings or {}

    def mkdir(self, path, parents=False):
        self.dirs.add(path)

    def write_file(self, path, content):
        self.files[path] = content if isinstance(content, bytes) else content.encode()

    def chmod(self, path, mode):
        self.modes[path] = mode

    def symlink(self, target, link_path):
        self.links[link_path] = target

    def read_file(self, path):
        return self.files[path]

    def readlink(self, path):
        return self.links[path]

    def list_dir(self, path):
        if path in self.listings:
            return self.listings[path]
        out = []
        now = datetime.now(timezone.utc)
        for p in sorted(set(self.files) | self.dirs | set(self.links)):
            pp = PurePosixPath(p)
            if str(pp.parent) == path.rstrip("/") or (path == "/" and str(pp.parent) == "/"):
                out.append(DirEntry(pp.name, p in self.dirs, p in self.links, len(self.files.get(p, b"")), self.modes.get(p, 0o644), now))
        return out


class AsyncFake:
    def __init__(self, inner: FakeSandbox):
        self.inner = inner

    def __getattr__(self, name):
        fn = getattr(self.inner, name)

        async def call(*a, **k):
            return fn(*a, **k)

        return call


def entry(name, is_dir=False, is_symlink=False, mode=0o644):
    return DirEntry(name, is_dir, is_symlink, 0, mode, datetime.now(timezone.utc))


# --- helpers -------------------------------------------------------------------


def test_safe_child_accepts_plain_names_and_rejects_everything_else():
    assert safe_child("/dest", "a.txt") == os.path.join("/dest", "a.txt")
    assert safe_child("/dest", "..hidden") == os.path.join("/dest", "..hidden")
    for bad in ("", ".", "..", "a/b", "../x", "/etc/passwd", "a\\b", "a\0b"):
        raises(ValueError, safe_child, "/dest", bad)


def test_symlink_stays_inside():
    with tempfile.TemporaryDirectory() as tmp:
        root = os.path.realpath(tmp)
        os.mkdir(os.path.join(root, "sub"))
        assert symlink_stays_inside(root, root, "sub")
        assert symlink_stays_inside(root, root, ".")
        assert symlink_stays_inside(root, os.path.join(root, "sub"), "../sub")
        assert symlink_stays_inside(root, root, "does/not/exist/yet")
        assert not symlink_stays_inside(root, root, "..")
        assert not symlink_stays_inside(root, root, "../elsewhere")
        assert not symlink_stays_inside(root, os.path.join(root, "sub"), "../../x")
        assert not symlink_stays_inside(root, root, "/etc/passwd")
        assert not symlink_stays_inside(root, root, "a\0b")
        # A name that only starts with the root's name is not inside it.
        assert not symlink_stays_inside(root, root, "../" + os.path.basename(root) + "-sibling")


def test_a_chain_of_harmless_links_cannot_add_up_to_an_escape():
    with tempfile.TemporaryDirectory() as tmp:
        root = os.path.realpath(tmp)
        os.mkdir(os.path.join(root, "x"))
        os.symlink("..", os.path.join(root, "x", "s"))  # x/s -> root, itself fine
        assert symlink_stays_inside(root, os.path.join(root, "x"), "..")
        # Lexically x/s/.. is x, but really it is the parent of root.
        assert not symlink_stays_inside(root, os.path.join(root, "x"), "s/..")


def test_duplicate_names_are_refused():
    check_unique_names(["a", "b"])
    raises(ValueError, check_unique_names, ["a", "b", "a"])


def test_posix_join():
    assert posix_join("/a", "b") == "/a/b" and posix_join("/a/", "b") == "/a/b" and posix_join("/", "b") == "/b"


def test_write_local_file_is_private_drops_special_bits_and_never_follows_a_link():
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "f")
        write_local_file(path, b"data", 0o4755)
        assert open(path, "rb").read() == b"data"
        assert os.stat(path).st_mode & 0o7777 == 0o755
        target = os.path.join(tmp, "target")
        open(target, "w").write("keep")
        os.symlink(target, os.path.join(tmp, "link"))
        raises(OSError, write_local_file, os.path.join(tmp, "link"), b"pwn", 0o644)
        assert open(target).read() == "keep"


# --- upload then download --------------------------------------------------------


def build_tree(root: str) -> None:
    os.makedirs(os.path.join(root, "src", "deep"))
    open(os.path.join(root, "README"), "w").write("hello")
    open(os.path.join(root, "src", "run.sh"), "w").write("#!/bin/sh\n")
    os.chmod(os.path.join(root, "src", "run.sh"), 0o755)
    open(os.path.join(root, "src", "deep", "blob.bin"), "wb").write(bytes(range(256)) * 10)
    open(os.path.join(root, "skip.log"), "w").write("noise")
    os.symlink("README", os.path.join(root, "link"))


def tree_snapshot(root: str) -> dict:
    out = {}
    for base, dirs, files in os.walk(root):
        for name in dirs + files:
            p = os.path.join(base, name)
            rel = os.path.relpath(p, root)
            if os.path.islink(p):
                out[rel] = ("link", os.readlink(p))
            elif os.path.isdir(p):
                out[rel] = ("dir",)
            else:
                out[rel] = ("file", open(p, "rb").read(), os.stat(p).st_mode & 0o777)
    return out


def test_upload_then_download_round_trips_content_modes_and_links():
    with tempfile.TemporaryDirectory() as src, tempfile.TemporaryDirectory() as dst:
        build_tree(src)
        box = FakeSandbox()
        up = upload_dir(box, src, "/work", exclude=lambda rel: rel.endswith(".log"), concurrency=3)
        assert (up.files, up.symlinks, up.skipped) == (3, 1, ["skip.log"])
        assert up.bytes == len("hello") + len("#!/bin/sh\n") + 2560
        assert up.directories == 3  # /work, src, src/deep
        assert box.files["/work/src/run.sh"] == b"#!/bin/sh\n" and box.modes["/work/src/run.sh"] == 0o755
        assert box.links["/work/link"] == "README"

        down = download_dir(box, "/work", os.path.join(dst, "out"))
        assert (down.files, down.symlinks, down.skipped) == (3, 1, [])
        expected = tree_snapshot(src)
        del expected["skip.log"]
        assert tree_snapshot(os.path.join(dst, "out")) == expected


def test_upload_rejects_a_bad_concurrency():
    raises(ValueError, upload_dir, FakeSandbox(), "/nonexistent", "/x", concurrency=0)


def test_async_round_trip_matches_the_sync_one():
    async def go(src, dst):
        box = FakeSandbox()
        up = await atransfer.upload_dir(AsyncFake(box), src, "/work", exclude=lambda rel: rel.endswith(".log"))
        down = await atransfer.download_dir(AsyncFake(box), "/work", dst)
        return up, down

    with tempfile.TemporaryDirectory() as src, tempfile.TemporaryDirectory() as dst:
        build_tree(src)
        up, down = asyncio.run(go(src, os.path.join(dst, "out")))
        assert (up.files, up.symlinks, down.files, down.symlinks) == (3, 1, 3, 1)
        expected = tree_snapshot(src)
        del expected["skip.log"]
        assert tree_snapshot(os.path.join(dst, "out")) == expected


# --- a hostile guest -----------------------------------------------------------------


def test_download_refuses_unsafe_names_from_the_guest():
    for bad in ("../escape", "a/b", "..", "/etc/cron.d/x"):
        with tempfile.TemporaryDirectory() as tmp:
            box = FakeSandbox(listings={"/work": [entry(bad)]})
            box.files["/work/" + bad] = b"pwn"
            dest = os.path.join(tmp, "dest")
            raises(ValueError, download_dir, box, "/work", dest)
            assert os.listdir(tmp) == ["dest"] and os.listdir(dest) == []


def test_download_skips_escaping_symlinks_and_keeps_inside_ones():
    with tempfile.TemporaryDirectory() as tmp:
        box = FakeSandbox(
            listings={
                "/work": [entry("ok", is_symlink=True), entry("up", is_symlink=True), entry("abs", is_symlink=True), entry("sub", is_dir=True)],
                "/work/sub": [entry("deep", is_symlink=True)],
            }
        )
        box.links.update({"/work/ok": "sub", "/work/up": "../outside", "/work/abs": "/etc/shadow", "/work/sub/deep": "../../outside"})
        dest = os.path.join(tmp, "dest")
        result = download_dir(box, "/work", dest)
        assert result.symlinks == 1 and sorted(result.skipped) == ["abs", "sub/deep", "up"]
        assert sorted(os.listdir(dest)) == ["ok", "sub"]
        assert os.readlink(os.path.join(dest, "ok")) == "sub"


def test_download_refuses_duplicate_names_so_a_link_cannot_redirect_a_later_write():
    with tempfile.TemporaryDirectory() as tmp:
        box = FakeSandbox(listings={"/work": [entry("f", is_symlink=True), entry("f")]})
        box.links["/work/f"] = "."
        box.files["/work/f"] = b"x"
        raises(ValueError, download_dir, box, "/work", os.path.join(tmp, "dest"))


def test_download_never_writes_a_file_through_a_symlink_already_on_disk():
    with tempfile.TemporaryDirectory() as tmp:
        dest = os.path.join(tmp, "dest")
        os.mkdir(dest)
        victim = os.path.join(tmp, "victim")
        open(victim, "w").write("keep")
        os.symlink(victim, os.path.join(dest, "f"))
        box = FakeSandbox(listings={"/work": [entry("f")]})
        box.files["/work/f"] = b"pwn"
        raises(OSError, download_dir, box, "/work", dest)
        assert open(victim).read() == "keep"


def test_download_will_not_descend_through_an_existing_symlinked_directory():
    with tempfile.TemporaryDirectory() as tmp:
        dest = os.path.join(tmp, "dest")
        outside = os.path.join(tmp, "outside")
        os.mkdir(dest)
        os.mkdir(outside)
        os.symlink(outside, os.path.join(dest, "d"))
        box = FakeSandbox(listings={"/work": [entry("d", is_dir=True)], "/work/d": [entry("f")]})
        box.files["/work/d/f"] = b"pwn"
        raises(ValueError, download_dir, box, "/work", dest)
        assert os.listdir(outside) == []


def test_download_masks_setuid_bits_from_an_untrusted_mode():
    with tempfile.TemporaryDirectory() as tmp:
        box = FakeSandbox(listings={"/work": [entry("f", mode=0o6777)]})
        box.files["/work/f"] = b"x"
        download_dir(box, "/work", os.path.join(tmp, "dest"))
        assert os.stat(os.path.join(tmp, "dest", "f")).st_mode & 0o7777 == 0o777


def run_all() -> int:
    failures = 0
    tests = [(n, f) for n, f in sorted(globals().items()) if n.startswith("test_") and callable(f)]
    for name, fn in tests:
        try:
            fn()
            print(f"ok   {name}")
        except BaseException as error:
            failures += 1
            print(f"FAIL {name}: {error!r}")
    print(f"{len(tests) - failures}/{len(tests)} passed")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(run_all())

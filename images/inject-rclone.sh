#!/usr/bin/env bash
# Bakes rclone plus fusermount3 into a copy of the test rootfs, at
# /usr/local/bin/{rclone,fusermount3}. Needs sudo (loop-mounts the ext4
# image). Same shape as inject-agent.sh: both are injected directly
# rather than installed via apt, since the test rootfs's own package
# manager can't be relied on — see routes_mounts.rs's module doc comment
# for why remote storage mounts need them at all, and
# images/build-guest-kernel.sh for the matching CONFIG_FUSE_FS kernel
# requirement.
#
# rclone's Linux FUSE backend (bazil.org/fuse) always execs
# `fusermount3` to perform the actual mount(2) call, even when already
# running as root inside the guest -- there's no direct-mount code path,
# so fusermount3 is a hard requirement, not just a convenience. It's not
# statically linked, but it only depends on libc (no libfuse3 needed):
# `ldd /bin/fusermount3` on any Debian/Ubuntu host with the `fuse3`
# package installed shows only libc.so.6 and the dynamic linker, both of
# which every guest already has.
#
# Usage: sudo images/inject-rclone.sh <rclone-binary> <rootfs-image> [fusermount3-binary]
#   fusermount3-binary defaults to /bin/fusermount3 on this build host
#   (install the `fuse3` package here if it's missing).

set -euo pipefail

RCLONE_BIN="${1:?path to a static rclone binary required}"
ROOTFS="${2:?path to the ext4 rootfs image required}"
FUSERMOUNT3_BIN="${3:-/bin/fusermount3}"

[ -x "$FUSERMOUNT3_BIN" ] || {
  echo "no fusermount3 binary at $FUSERMOUNT3_BIN -- install the fuse3 package on this host, or pass a path explicitly" >&2
  exit 1
}

MNT="$(mktemp -d)"
cleanup() {
  umount "$MNT" 2>/dev/null || true
  rmdir "$MNT" 2>/dev/null || true
}
trap cleanup EXIT

mount -o loop "$ROOTFS" "$MNT"

install -m 0755 "$RCLONE_BIN" "$MNT/usr/local/bin/rclone"
install -m 0755 "$FUSERMOUNT3_BIN" "$MNT/usr/local/bin/fusermount3"

echo "rclone + fusermount3 injected into $ROOTFS"

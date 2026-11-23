#!/usr/bin/env bash
# Creates an XFS (reflink-enabled) loopback volume for sandkiln's rootfs
# clones, on hosts whose main filesystem has no reflink support (ext4).
#
# Reflink makes a per-sandbox rootfs clone near-instant and space-free
# instead of a full byte copy. The base image and the clone directory must
# be on the SAME filesystem, so this script also copies the base image in.
#
# Usage: sudo scripts/host-setup/create-reflink-volume.sh <owner-user> [size-gib] [mountpoint] [backing-file]
# Example: sudo scripts/host-setup/create-reflink-volume.sh t1000 20
#
# Then run the daemon with:
#   SANDKILN_BASE_ROOTFS=<mountpoint>/<image>.ext4 SANDKILN_ROOTFS_DIR=<mountpoint>/rootfs
# and check `rootfs_clone_total{method="reflink"}` on /metrics.
#
# Not persistent across reboots (loop mounts aren't); re-run to remount. Safe to re-run.

set -euo pipefail

OWNER="${1:?owning username required}"
SIZE_GIB="${2:-20}"
MOUNTPOINT="${3:-/mnt/sandkiln-xfs}"
BACKING="${4:-/var/lib/sandkiln-xfs.img}"

[ "$(id -u)" -eq 0 ] || { echo "must run as root: sudo $0 $*" >&2; exit 1; }
command -v mkfs.xfs >/dev/null || { echo "mkfs.xfs not found (apt-get install xfsprogs)" >&2; exit 1; }

if mountpoint -q "$MOUNTPOINT"; then
  echo "already mounted: $MOUNTPOINT"
else
  if [ ! -f "$BACKING" ]; then
    truncate -s "${SIZE_GIB}G" "$BACKING"
    mkfs.xfs -q -m reflink=1 "$BACKING"
  fi
  mkdir -p "$MOUNTPOINT"
  mount -o loop "$BACKING" "$MOUNTPOINT"
fi
mkdir -p "$MOUNTPOINT/rootfs"
chown -R "$OWNER": "$MOUNTPOINT"
echo "reflink volume ready at $MOUNTPOINT ($(findmnt -no FSTYPE "$MOUNTPOINT"))"

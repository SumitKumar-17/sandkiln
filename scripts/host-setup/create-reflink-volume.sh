#!/usr/bin/env bash
# Creates an XFS (reflink-enabled) loopback volume for sandkiln's rootfs
# clones, on hosts whose main filesystem has no reflink support (ext4).
#
# Reflink makes a per-sandbox rootfs clone near-instant and space-free
# instead of a full byte copy. The base image and the clone directory must
# be on the SAME filesystem, so pass the base image to copy it onto the volume.
#
# Usage: sudo scripts/host-setup/create-reflink-volume.sh <owner-user> [size-gib] [mountpoint] [backing-file] [base-image]
# Example: sudo scripts/host-setup/create-reflink-volume.sh t1000 20 /mnt/sandkiln-xfs /var/lib/sandkiln-xfs.img ~/sandkiln-tools/images/ubuntu-22.04.ext4
#
# Then set in sandkiln.toml (or the matching SANDKILN_* env vars):
#   base_rootfs = "<mountpoint>/base.ext4"
#   rootfs_dir  = "<mountpoint>/rootfs"
# and check `rootfs_clone_total{method="reflink"}` on /metrics.
#
# Not persistent across reboots (loop mounts aren't); re-run to remount. Safe to re-run.

set -euo pipefail

OWNER="${1:?owning username required}"
SIZE_GIB="${2:-20}"
MOUNTPOINT="${3:-/mnt/sandkiln-xfs}"
BACKING="${4:-/var/lib/sandkiln-xfs.img}"
BASE_IMAGE="${5:-}"

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
if [ -n "$BASE_IMAGE" ]; then
  [ -f "$BASE_IMAGE" ] || { echo "base image not found: $BASE_IMAGE" >&2; exit 1; }
  cp --reflink=auto "$BASE_IMAGE" "$MOUNTPOINT/base.ext4"
  echo "base image copied to $MOUNTPOINT/base.ext4"
fi
chown -R "$OWNER": "$MOUNTPOINT"
echo "reflink volume ready at $MOUNTPOINT ($(findmnt -no FSTYPE "$MOUNTPOINT"))"

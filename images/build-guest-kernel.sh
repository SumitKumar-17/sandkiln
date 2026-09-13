#!/usr/bin/env bash
# Builds a FUSE-capable guest kernel for use with SANDKILN_KERNEL_PATH.
#
# Firecracker's own default/CI guest kernel configs do not enable
# CONFIG_FUSE_FS, and guest kernels need every feature statically
# compiled in (no loadable module support) -- this is a real, documented
# upstream constraint, not a sandkiln limitation. Remote storage mounts
# (routes_mounts.rs, via `rclone mount`) need /dev/fuse to exist inside
# the guest, so a stock Firecracker kernel can't run that feature at all.
#
# This script rebuilds vmlinux from vanilla kernel source using
# Firecracker's own published recommended config (which does enable
# CONFIG_FUSE_FS) as the base, reconciled against the target kernel
# version with `make olddefconfig`. Needs a build toolchain: gcc, make,
# flex, bison, libelf-dev, bc (`sudo apt-get install -y build-essential
# flex bison libelf-dev bc`).
#
# Usage: images/build-guest-kernel.sh [kernel-version] [output-path]
#   kernel-version defaults to 5.10.223 (this project's currently
#   deployed guest kernel, for continuity -- see .env.sandkiln-setup).
#   output-path defaults to ./vmlinux-<kernel-version>-fuse

set -euo pipefail

KERNEL_VERSION="${1:-5.10.223}"
OUTPUT="${2:-./vmlinux-${KERNEL_VERSION}-fuse}"
WORKDIR="$(mktemp -d)"
trap 'rm -rf "$WORKDIR"' EXIT

echo "==> fetching Firecracker's recommended x86_64 5.10 guest kernel config"
curl -sL -o "$WORKDIR/fc.config" \
  "https://raw.githubusercontent.com/firecracker-microvm/firecracker/main/resources/guest_configs/microvm-kernel-ci-x86_64-5.10.config"

echo "==> fetching kernel source $KERNEL_VERSION"
KERNEL_MAJOR="${KERNEL_VERSION%%.*}"
curl -sL -o "$WORKDIR/linux.tar.xz" \
  "https://cdn.kernel.org/pub/linux/kernel/v${KERNEL_MAJOR}.x/linux-${KERNEL_VERSION}.tar.xz"

echo "==> extracting"
tar -xf "$WORKDIR/linux.tar.xz" -C "$WORKDIR"
SRC="$WORKDIR/linux-${KERNEL_VERSION}"

cp "$WORKDIR/fc.config" "$SRC/.config"

echo "==> reconciling config against $KERNEL_VERSION (make olddefconfig)"
make -C "$SRC" olddefconfig

grep -q '^CONFIG_FUSE_FS=y' "$SRC/.config" || {
  echo "CONFIG_FUSE_FS did not survive olddefconfig -- refusing to build" >&2
  exit 1
}

echo "==> building vmlinux (this takes a few minutes)"
make -C "$SRC" vmlinux -j"$(nproc)"

cp "$SRC/vmlinux" "$OUTPUT"
echo "==> built $OUTPUT"
echo "Point SANDKILN_KERNEL_PATH at it to use it, e.g.:"
echo "  SANDKILN_KERNEL_PATH=$(readlink -f "$OUTPUT") scripts/sandkilnd-ctl.sh start"

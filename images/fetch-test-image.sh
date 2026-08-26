#!/usr/bin/env bash
# Fetches a known-good kernel + ext4 rootfs from Firecracker's public CI
# artifact bucket, for manual boot testing (Phase 1). Not a production
# image source — Phase 6 replaces this with our own built images.

set -euo pipefail

DEST="${1:-$HOME/sandkiln-tools/images}"
BASE="https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.10/x86_64"

mkdir -p "$DEST"
cd "$DEST"
curl -L -o vmlinux-5.10.223 "$BASE/vmlinux-5.10.223"
curl -L -o ubuntu-22.04.ext4 "$BASE/ubuntu-22.04.ext4"
curl -L -o ubuntu-22.04.id_rsa "$BASE/ubuntu-22.04.id_rsa"
chmod 600 ubuntu-22.04.id_rsa

echo "kernel + rootfs ready in $DEST"

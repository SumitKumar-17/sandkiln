#!/usr/bin/env bash
# Downloads the Firecracker + jailer binaries into ~/sandkiln-tools/bin.
# Requires KVM access (/dev/kvm, rw) on the host running this.

set -euo pipefail

VERSION="${FIRECRACKER_VERSION:-v1.16.1}"
ARCH="$(uname -m)"
DEST="${1:-$HOME/sandkiln-tools}"

mkdir -p "$DEST/bin"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

curl -L -o "$TMP/firecracker.tgz" \
  "https://github.com/firecracker-microvm/firecracker/releases/download/${VERSION}/firecracker-${VERSION}-${ARCH}.tgz"
tar xzf "$TMP/firecracker.tgz" -C "$TMP"

RELEASE_DIR="$TMP/release-${VERSION}-${ARCH}"
cp "$RELEASE_DIR/firecracker-${VERSION}-${ARCH}" "$DEST/bin/firecracker"
cp "$RELEASE_DIR/jailer-${VERSION}-${ARCH}" "$DEST/bin/jailer"
chmod +x "$DEST/bin/firecracker" "$DEST/bin/jailer"

"$DEST/bin/firecracker" --version

#!/usr/bin/env bash
# Boots a single Firecracker microVM by hand, driving it purely through its
# API socket. This is the Phase 1 proof that the primitive works — no
# daemon, no SDK yet. Networking is optional: pass a tap device name (set
# up via setup-tap-network.sh) to give the guest outbound internet at a
# fixed static IP. Blocks with the console attached; the guest logs in as
# root automatically. Stop the VM from another shell with
# `kill <firecracker pid>` or SendCtrlAltDel over the API.
#
# This test rootfs doesn't configure DNS on its own — once booted, set
# `nameserver <host-ip>` in the guest's /etc/resolv.conf (start-dns-proxy.sh
# needs to be running on the host side first). Phase 2's own rootfs bakes
# this in at build time instead.
#
# Usage: scripts/boot-test-vm.sh [images-dir] [tap-device]
# Example: scripts/boot-test-vm.sh ~/sandkiln-tools/images fc-tap0

set -euo pipefail

IMAGES="${1:-$HOME/sandkiln-tools/images}"
TAP="${2:-}"
ROOTFS_SRC="${3:-$IMAGES/ubuntu-22.04.ext4}"
FC="${FIRECRACKER_BIN:-$HOME/sandkiln-tools/bin/firecracker}"
SOCK="$(mktemp -u /tmp/sandkiln-fc-XXXXXX.sock)"
ROOTFS="$(mktemp -u /tmp/sandkiln-rootfs-XXXXXX.ext4)"
VSOCK="$(mktemp -u /tmp/sandkiln-vsock-XXXXXX.sock)"

# Matches the host-side address configured by setup-tap-network.sh
# (172.16.0.1/24) — guest takes .2 on the same /24.
GUEST_IP="172.16.0.2"
HOST_IP="172.16.0.1"
GUEST_MAC="AA:FC:00:00:00:01"

cp "$ROOTFS_SRC" "$ROOTFS"

cleanup() {
  rm -f "$SOCK" "$ROOTFS" "$VSOCK"
}
trap cleanup EXIT

"$FC" --api-sock "$SOCK" &
FC_PID=$!
sleep 1

BOOT_ARGS="console=ttyS0 reboot=k panic=1 pci=off"
if [ -n "$TAP" ]; then
  BOOT_ARGS="$BOOT_ARGS ip=${GUEST_IP}::${HOST_IP}:255.255.255.0::eth0:off"
fi

curl -s --unix-socket "$SOCK" -X PUT "http://localhost/boot-source" \
  -H "Content-Type: application/json" \
  -d "{\"kernel_image_path\": \"$IMAGES/vmlinux-5.10.223\", \"boot_args\": \"$BOOT_ARGS\"}"

curl -s --unix-socket "$SOCK" -X PUT "http://localhost/drives/rootfs" \
  -H "Content-Type: application/json" \
  -d "{\"drive_id\": \"rootfs\", \"path_on_host\": \"$ROOTFS\", \"is_root_device\": true, \"is_read_only\": false}"

curl -s --unix-socket "$SOCK" -X PUT "http://localhost/machine-config" \
  -H "Content-Type: application/json" \
  -d '{"vcpu_count": 2, "mem_size_mib": 512}'

if [ -n "$TAP" ]; then
  curl -s --unix-socket "$SOCK" -X PUT "http://localhost/network-interfaces/eth0" \
    -H "Content-Type: application/json" \
    -d "{\"iface_id\": \"eth0\", \"guest_mac\": \"$GUEST_MAC\", \"host_dev_name\": \"$TAP\"}"
fi

curl -s --unix-socket "$SOCK" -X PUT "http://localhost/vsock" \
  -H "Content-Type: application/json" \
  -d "{\"vsock_id\": \"vsock0\", \"guest_cid\": 3, \"uds_path\": \"$VSOCK\"}"

curl -s --unix-socket "$SOCK" -X PUT "http://localhost/actions" \
  -H "Content-Type: application/json" \
  -d '{"action_type": "InstanceStart"}'

echo "booted. firecracker pid: $FC_PID, api socket: $SOCK, vsock uds: $VSOCK, guest ip: ${TAP:+$GUEST_IP}" >&2
wait "$FC_PID"

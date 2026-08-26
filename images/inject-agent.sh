#!/usr/bin/env bash
# Bakes the guest agent binary into a copy of the test rootfs and enables
# it as a systemd service, so it starts automatically at boot. Needs sudo
# (loop-mounts the ext4 image).
#
# Usage: sudo images/inject-agent.sh <agent-binary> <rootfs-image>

set -euo pipefail

AGENT_BIN="${1:?path to the built guest agent binary required}"
ROOTFS="${2:?path to the ext4 rootfs image required}"

MNT="$(mktemp -d)"
cleanup() {
  umount "$MNT" 2>/dev/null || true
  rmdir "$MNT" 2>/dev/null || true
}
trap cleanup EXIT

mount -o loop "$ROOTFS" "$MNT"

install -m 0755 "$AGENT_BIN" "$MNT/usr/local/bin/sandkiln-agent"

cat > "$MNT/etc/systemd/system/sandkiln-agent.service" <<'EOF'
[Unit]
Description=sandkiln guest agent
After=network.target

[Service]
ExecStart=/usr/local/bin/sandkiln-agent
Restart=always
Type=simple

[Install]
WantedBy=multi-user.target
EOF

systemctl --root="$MNT" enable sandkiln-agent.service

echo "agent injected and enabled in $ROOTFS"

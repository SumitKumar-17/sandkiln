#!/usr/bin/env bash
# Bakes the guest agent binary into a copy of the test rootfs and enables
# it as a systemd service, so it starts automatically at boot. Needs sudo
# (loop-mounts the ext4 image).
#
# With --init the agent becomes the guest's PID 1 instead: /sbin/init is
# pointed at it (the kernel runs /sbin/init when no `init=` is given), the
# agent mounts the pseudo filesystems, sets the hostname, brings loopback up
# and reaps orphans itself, and systemd never runs. The original init is kept
# as /sbin/init.systemd. Run it on a COPY of an image to get a second,
# opt-in image; re-running without --init on the same file restores systemd.
#
# Usage: sudo images/inject-agent.sh [--init] <agent-binary> <rootfs-image>

set -euo pipefail

INIT_MODE=0
if [ "${1:-}" = "--init" ]; then
  INIT_MODE=1
  shift
fi

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

AGENT_PATH=/usr/local/bin/sandkiln-agent

# chroot so a /sbin that is an absolute symlink (usr-merged images) resolves
# inside the image, not on the host.
if [ "$INIT_MODE" = 1 ]; then
  current="$(chroot "$MNT" readlink -f /sbin/init)"
  if [ "$current" != "$AGENT_PATH" ] && ! [ -e "$MNT/sbin/init.systemd" ]; then
    chroot "$MNT" ln -sfn "$(chroot "$MNT" readlink /sbin/init)" /sbin/init.systemd
  fi
  chroot "$MNT" ln -sfn "$AGENT_PATH" /sbin/init
  echo "agent injected as PID 1 (/sbin/init) in $ROOTFS; systemd kept as /sbin/init.systemd"
  exit 0
fi

# Back to systemd if this image was previously an --init image.
if [ -L "$MNT/sbin/init.systemd" ]; then
  chroot "$MNT" mv -f /sbin/init.systemd /sbin/init
fi

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

#!/usr/bin/env bash
# Bakes multi-agent isolation into a rootfs image: N sequential agent user
# accounts, each with a private home directory, plus a shared group and
# shared directory for deliberate cross-agent file sharing. See
# "Multi-agent isolation" in ROADMAP.md.
#
# Called automatically by build-universal-image.sh, but also standalone
# against any already-built ext4 rootfs image (the same test image fetched
# by fetch-test-image.sh, a custom image, whatever) — same convention as
# inject-agent.sh: mounts the finished image itself, needs sudo.
#
# UID/GID convention (documented here because future code — e.g. a daemon
# feature that lets a caller pick which agent user an `exec` runs as —
# needs to reference it):
#   - Agent usernames are `agent0`, `agent1`, ... `agent<N-1>`, sequential
#     and predictable.
#   - Agent UIDs start at UID_BASE (default 2000) and increment by 1 per
#     agent: agent0 = 2000, agent1 = 2001, etc. UID N maps back to
#     username via `agent$((N - UID_BASE))`. 2000 is chosen to sit clear
#     of both system UIDs (<1000) and a machine's normal first interactive
#     user (usually 1000) — it's a reserved range for sandkiln agent
#     users specifically.
#   - Each agent gets its own private group (same name, matching GID) for
#     its home directory, which is mode 700 — private by default, not
#     just "not world-writable".
#   - All agent users are also members of a single shared group, `shared`,
#     fixed at GID 5000 (a separate range from the agent UID/GID block so
#     the two never collide as agent count grows).
#   - `/srv/shared` is owned root:shared, mode 2775 (setgid, so files
#     created inside it inherit the shared group) — the deliberate,
#     opt-in sharing surface between agents on the same sandbox.
#   - Agent accounts are locked (no password) — they're reached via the
#     guest agent/exec path, not interactive login.
#
# Usage:
#   sudo images/setup-multi-agent-users.sh <rootfs-image> [agent-count] [uid-base] [shared-gid]
# Example:
#   sudo images/setup-multi-agent-users.sh ~/sandkiln-tools/images/universal.ext4 4

set -euo pipefail

ROOTFS="${1:?path to the ext4 rootfs image required}"
AGENT_COUNT="${2:-4}"
UID_BASE="${3:-2000}"
SHARED_GID="${4:-5000}"
SHARED_GROUP="shared"

if [[ $EUID -ne 0 ]]; then
  echo "must run as root (sudo) — loop-mounts the image and edits its user database" >&2
  exit 1
fi

for tool in useradd groupadd usermod mount; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing required tool: $tool" >&2; exit 1; }
done

if ! [[ "$AGENT_COUNT" =~ ^[0-9]+$ ]] || [[ "$AGENT_COUNT" -lt 1 ]]; then
  echo "agent-count must be a positive integer, got: $AGENT_COUNT" >&2
  exit 1
fi

MNT="$(mktemp -d)"
cleanup() {
  umount "$MNT" 2>/dev/null || umount -l "$MNT" 2>/dev/null || true
  rmdir "$MNT" 2>/dev/null || true
}
trap cleanup EXIT

mount -o loop "$ROOTFS" "$MNT"

if [[ ! -e "$MNT/etc/passwd" ]]; then
  echo "$ROOTFS doesn't look like a Linux rootfs (no /etc/passwd)" >&2
  exit 1
fi

# --root makes useradd/groupadd/usermod operate offline against the target
# image's own /etc/passwd, group, shadow — no chroot, no bind-mounting
# /proc or /dev needed. Same trick inject-agent.sh already relies on via
# `systemctl --root`.

echo "==> shared group (gid $SHARED_GID)"
if ! grep -q "^${SHARED_GROUP}:" "$MNT/etc/group"; then
  groupadd --root "$MNT" --gid "$SHARED_GID" "$SHARED_GROUP"
else
  echo "$SHARED_GROUP already exists, skipping"
fi

echo "==> /srv/shared"
mkdir -p "$MNT/srv/shared"
# chown/chmod act on the mounted filesystem directly and don't go through
# --root, so use the numeric GID here, not the name "shared" — a plain
# `chown` resolves group names via the HOST's /etc/group, not the target
# image's, and the two are unrelated databases.
chown 0:"$SHARED_GID" "$MNT/srv/shared"
chmod 2775 "$MNT/srv/shared"

echo "==> agent users ($AGENT_COUNT, uid $UID_BASE..$((UID_BASE + AGENT_COUNT - 1)))"
for ((i = 0; i < AGENT_COUNT; i++)); do
  agent_user="agent${i}"
  agent_uid=$((UID_BASE + i))
  home="/home/${agent_user}"

  if grep -q "^${agent_user}:" "$MNT/etc/passwd"; then
    echo "$agent_user already exists, skipping creation"
  else
    useradd --root "$MNT" \
      --uid "$agent_uid" \
      --user-group \
      --create-home \
      --home-dir "$home" \
      --shell /bin/bash \
      "$agent_user"
    # Locked account: no password, no interactive login expected — the
    # guest agent/exec path is how these users get used.
  fi

  # Idempotent regardless of whether the user was just created or already
  # existed (e.g. a previous run predates the shared group).
  usermod --root "$MNT" -aG "$SHARED_GROUP" "$agent_user"

  # Private home: 700, not whatever the distro's default HOME_MODE is —
  # this is the actual isolation guarantee, don't rely on a default.
  chmod 700 "$MNT$home"
done

echo "$AGENT_COUNT agent users ready in $ROOTFS: agent0..agent$((AGENT_COUNT - 1)), shared group '$SHARED_GROUP' (gid $SHARED_GID), /srv/shared"

#!/usr/bin/env bash
# Builds the production universal base rootfs from scratch with debootstrap:
# Ubuntu + current Node.js LTS + Python 3 + common CLI tooling + a working
# systemd init, as an ext4 image ready to boot under Firecracker. This
# replaces the stripped-down Firecracker CI test image (fetched by
# fetch-test-image.sh) as SANDKILN_BASE_ROOTFS — that test image is missing
# even ca-certificates and was never meant to back real sandboxes. See
# "Base and custom images" in ROADMAP.md.
#
# Needs sudo: debootstrap, loop-mounting the image, and chroot operations
# all require real root, the same as scripts/create-tap-pool.sh and
# scripts/grant-net-admin.sh.
#
# Usage:
#   sudo images/build-universal-image.sh <output-image-path> [size-in-gb] [ubuntu-codename] [agent-count]
# Example:
#   sudo images/build-universal-image.sh ~/sandkiln-tools/images/universal.ext4 6 resolute 4
#
# Env overrides:
#   UBUNTU_MIRROR   apt/debootstrap mirror (default: http://archive.ubuntu.com/ubuntu)
#   NODE_MAJOR      Node.js major release line to install (default: 24, the
#                    Active LTS line as of 2026 — bump this as Node's LTS
#                    line moves on: https://nodejs.org/en/about/previous-releases)
#
# After building, inject the guest agent unmodified:
#   cargo build --release -p sandkiln-guest-agent --target x86_64-unknown-linux-musl
#   sudo images/inject-agent.sh target/x86_64-unknown-linux-musl/release/sandkiln-agent <output-image-path>

set -euo pipefail

# ---------------------------------------------------------------------------
# Args and config
# ---------------------------------------------------------------------------

OUTPUT="${1:?output image path required, e.g. ~/sandkiln-tools/images/universal.ext4}"
SIZE_GB="${2:-6}"
# "resolute" = Ubuntu 26.04 LTS (Resolute Raccoon), the current LTS as of
# 2026. If your debootstrap doesn't know this codename yet (it ships one
# script per release and only picks up new ones on package update), either
# `apt-get install --only-upgrade debootstrap` or pass an older LTS
# codename explicitly, e.g. `noble` for 24.04.
CODENAME="${3:-resolute}"
AGENT_COUNT="${4:-4}"

MIRROR="${UBUNTU_MIRROR:-http://archive.ubuntu.com/ubuntu}"
NODE_MAJOR="${NODE_MAJOR:-24}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Matches the daemon's default SANDKILN_BRIDGE_GATEWAY (see
# core/crates/daemon/src/config.rs) and start-dns-proxy.sh's listen
# address. boot-test-vm.sh notes the CI test image doesn't bake in DNS at
# all and a production rootfs should — this is that: every sandbox that
# boots from this image gets working DNS out of the box, forwarded through
# whichever host is running start-dns-proxy.sh on this address. Override
# if your dev box's bridge gateway differs from the project default.
GUEST_NAMESERVER="${GUEST_NAMESERVER:-172.16.0.1}"

# ---------------------------------------------------------------------------
# Preflight
# ---------------------------------------------------------------------------

if [[ $EUID -ne 0 ]]; then
  echo "must run as root (sudo) — debootstrap and chroot operations need it" >&2
  exit 1
fi

if [[ "$(uname -m)" != "x86_64" ]]; then
  echo "must run on an x86_64 host — Firecracker guests in this project are x86_64" >&2
  exit 1
fi

for tool in debootstrap mkfs.ext4 mount chroot curl useradd; do
  command -v "$tool" >/dev/null 2>&1 || { echo "missing required tool: $tool" >&2; exit 1; }
done

if [[ ! -e "/usr/share/debootstrap/scripts/$CODENAME" ]]; then
  echo "debootstrap doesn't have a script for codename '$CODENAME'" >&2
  echo "(checked /usr/share/debootstrap/scripts/$CODENAME)" >&2
  echo "either upgrade debootstrap (apt-get install --only-upgrade debootstrap)" >&2
  echo "or pass an older LTS codename explicitly, e.g.:" >&2
  echo "  sudo $0 $OUTPUT $SIZE_GB noble" >&2
  exit 1
fi

# debootstrap verifies the Ubuntu archive's Release file against this
# keyring. It's normally pulled in by the `ubuntu-keyring` package — present
# by default on an Ubuntu host, but not necessarily on e.g. a Debian host.
# Fail loudly here instead of letting debootstrap die deep into a GPG error.
if [[ ! -e /usr/share/keyrings/ubuntu-archive-keyring.gpg ]]; then
  echo "missing /usr/share/keyrings/ubuntu-archive-keyring.gpg on this host" >&2
  echo "install it with: apt-get install ubuntu-keyring" >&2
  exit 1
fi

if [[ -e "$OUTPUT" ]]; then
  echo "refusing to overwrite existing file: $OUTPUT (remove it first if you mean to rebuild)" >&2
  exit 1
fi
mkdir -p "$(dirname "$OUTPUT")"

if [[ ! -x "$SCRIPT_DIR/setup-multi-agent-users.sh" ]]; then
  echo "missing or non-executable: $SCRIPT_DIR/setup-multi-agent-users.sh" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Cleanup machinery — must never leave a half-built mounted chroot or a
# corrupt partial image lying around, no matter where a failure happens.
# ---------------------------------------------------------------------------

MNT="$(mktemp -d)"
BUILD_OK=0

unmount_all() {
  # Safe to call more than once: mountpoint checks make every step a no-op
  # once already unmounted, so calling this mid-script (to free the image
  # before handing it to setup-multi-agent-users.sh) and again from the
  # EXIT trap is fine.
  for sub in dev/pts dev proc sys; do
    if mountpoint -q "$MNT/$sub" 2>/dev/null; then
      umount "$MNT/$sub" 2>/dev/null || umount -l "$MNT/$sub" 2>/dev/null || true
    fi
  done
  if mountpoint -q "$MNT" 2>/dev/null; then
    umount "$MNT" 2>/dev/null || umount -l "$MNT" 2>/dev/null || true
  fi
}

cleanup() {
  local ec=$?
  set +e
  unmount_all
  rmdir "$MNT" 2>/dev/null
  if [[ "$BUILD_OK" -ne 1 ]]; then
    echo "build did not complete — removing partial image $OUTPUT" >&2
    rm -f "$OUTPUT"
  fi
  exit "$ec"
}
trap cleanup EXIT
trap 'echo "ERROR: build-universal-image.sh failed at line $LINENO" >&2' ERR

# ---------------------------------------------------------------------------
# Create and format the image
# ---------------------------------------------------------------------------

echo "==> creating ${SIZE_GB}G sparse image at $OUTPUT"
truncate -s "${SIZE_GB}G" "$OUTPUT"
mkfs.ext4 -F -L sandkiln-rootfs "$OUTPUT" >/dev/null

echo "==> mounting"
mount -o loop "$OUTPUT" "$MNT"

# ---------------------------------------------------------------------------
# Bind mounts, before debootstrap runs — its second stage configures
# packages (dpkg maintainer scripts) inside the chroot via an internal
# chroot of its own, and those can expect /proc, /dev, /sys to already be
# there. Standard practice for building a chroot this way. The filesystem
# is freshly mkfs'd and otherwise completely empty (debootstrap normally
# creates these directories itself), so mkdir -p them first as mount
# points.
# ---------------------------------------------------------------------------

mkdir -p "$MNT/dev" "$MNT/proc" "$MNT/sys" "$MNT/etc"
mount --bind /dev "$MNT/dev"
mount --bind /proc "$MNT/proc"
mount --bind /sys "$MNT/sys"
cp /etc/resolv.conf "$MNT/etc/resolv.conf"

# ---------------------------------------------------------------------------
# debootstrap
# ---------------------------------------------------------------------------

echo "==> debootstrap: $CODENAME from $MIRROR"
debootstrap --arch=amd64 --components=main,universe,restricted,multiverse \
  "$CODENAME" "$MNT" "$MIRROR"

# Full four-component sources so `apt install` works for anything in the
# Ubuntu archive once a sandbox is running, not just what debootstrap
# itself needed. Ubuntu 24.04+ default to the deb822 sources format
# (/etc/apt/sources.list.d/ubuntu.sources) instead of the classic
# one-line-per-suite sources.list — match that.
rm -f "$MNT/etc/apt/sources.list"
cat > "$MNT/etc/apt/sources.list.d/ubuntu.sources" <<EOF
Types: deb
URIs: $MIRROR
Suites: $CODENAME $CODENAME-updates $CODENAME-security
Components: main restricted universe multiverse
Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
EOF

# ---------------------------------------------------------------------------
# Package install, Node.js, locale/timezone — all inside the chroot
# ---------------------------------------------------------------------------

cat > "$MNT/root/sandkiln-setup.sh" <<'SETUP_EOF'
#!/bin/bash
# Runs INSIDE the chroot via `chroot $MNT env NODE_MAJOR=... bash
# /root/sandkiln-setup.sh` — not meant to be run directly outside one.
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
export LC_ALL=C

echo "--> apt-get update"
apt-get update

echo "--> installing base packages"
# ca-certificates fixes the exact gap the old Firecracker CI test image
# had (no CA certs at all, so any HTTPS call inside a sandbox failed).
# python-is-python3 symlinks `python` -> python3 for tools/scripts that
# still assume the unversioned name.
apt-get install -y --no-install-recommends \
  ca-certificates \
  curl \
  wget \
  git \
  build-essential \
  python3 \
  python3-pip \
  python3-venv \
  python3-dev \
  python-is-python3 \
  sudo \
  locales \
  tzdata \
  iproute2 \
  iputils-ping \
  openssh-client \
  rsync \
  jq \
  less \
  nano \
  tar \
  unzip \
  xz-utils \
  gnupg \
  systemd \
  systemd-sysv \
  dbus

echo "--> locale"
sed -i 's/^# *\(en_US\.UTF-8 UTF-8\)/\1/' /etc/locale.gen
locale-gen
update-locale LANG=en_US.UTF-8

echo "--> timezone"
ln -sf /usr/share/zoneinfo/UTC /etc/localtime
echo "Etc/UTC" > /etc/timezone
dpkg-reconfigure -f noninteractive tzdata

echo "--> installing Node.js ${NODE_MAJOR}.x LTS"
NODE_DIST_URL="https://nodejs.org/dist/latest-v${NODE_MAJOR}.x"
NODE_TARBALL="$(curl -fsSL "$NODE_DIST_URL/" | grep -oE "node-v${NODE_MAJOR}\.[0-9]+\.[0-9]+-linux-x64\.tar\.xz" | head -n1)"
if [ -z "$NODE_TARBALL" ]; then
  echo "could not find a linux-x64 tarball for Node.js ${NODE_MAJOR}.x at $NODE_DIST_URL" >&2
  exit 1
fi
curl -fsSL -o "/tmp/${NODE_TARBALL}" "$NODE_DIST_URL/${NODE_TARBALL}"
curl -fsSL -o /tmp/SHASUMS256.txt "$NODE_DIST_URL/SHASUMS256.txt"
( cd /tmp && grep " ${NODE_TARBALL}\$" SHASUMS256.txt | sha256sum -c - )
tar -xJf "/tmp/${NODE_TARBALL}" -C /usr/local --strip-components=1
rm -f "/tmp/${NODE_TARBALL}" /tmp/SHASUMS256.txt
node --version
npm --version

echo "--> disabling background apt timers"
# An ephemeral sandbox has no business running unattended-upgrades or
# apt-daily in the background during its lifetime — just noise and
# unpredictable network activity.
systemctl mask apt-daily.timer apt-daily-upgrade.timer apt-daily.service apt-daily-upgrade.service 2>/dev/null || true

echo "--> cleanup"
apt-get clean
rm -rf /var/lib/apt/lists/*
SETUP_EOF
chmod +x "$MNT/root/sandkiln-setup.sh"

# debootstrap manages /proc itself internally during its own second stage
# and may leave it unmounted afterwards — make sure our three bind mounts
# are actually still there before chrooting in ourselves.
for sub in dev proc sys; do
  mountpoint -q "$MNT/$sub" || mount --bind "/$sub" "$MNT/$sub"
done

echo "==> running package install inside chroot (this takes a while)"
chroot "$MNT" env NODE_MAJOR="$NODE_MAJOR" /bin/bash /root/sandkiln-setup.sh
rm -f "$MNT/root/sandkiln-setup.sh"

# ---------------------------------------------------------------------------
# Base system config: hostname, hosts, runtime DNS, serial console autologin
# ---------------------------------------------------------------------------

echo "==> base system config"
echo "sandkiln-sandbox" > "$MNT/etc/hostname"
cat > "$MNT/etc/hosts" <<EOF
127.0.0.1   localhost
127.0.1.1   sandkiln-sandbox
EOF

# Build-time resolv.conf (copied from the host above) is replaced here with
# the guest's actual runtime nameserver — see GUEST_NAMESERVER above.
cat > "$MNT/etc/resolv.conf" <<EOF
nameserver $GUEST_NAMESERVER
EOF

# systemd's getty generator starts serial-getty@ttyS0 automatically from
# the kernel's console=ttyS0 boot arg (see boot-test-vm.sh) — no explicit
# `systemctl enable` needed for the unit itself. This override just adds
# autologin, matching the CI test image's "logs in as root automatically"
# console behavior for manual debugging via boot-test-vm.sh.
mkdir -p "$MNT/etc/systemd/system/serial-getty@ttyS0.service.d"
cat > "$MNT/etc/systemd/system/serial-getty@ttyS0.service.d/autologin.conf" <<'EOF'
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin root --keep-baud 115200,38400,9600 %I $TERM
EOF

# ---------------------------------------------------------------------------
# Unmount before handing off to setup-multi-agent-users.sh, which does its
# own independent mount of the finished image (same convention as
# inject-agent.sh: operate on a closed rootfs file, not a live mount).
# ---------------------------------------------------------------------------

echo "==> unmounting"
unmount_all

echo "==> setting up multi-agent isolation users ($AGENT_COUNT agents)"
"$SCRIPT_DIR/setup-multi-agent-users.sh" "$OUTPUT" "$AGENT_COUNT"

BUILD_OK=1
echo "universal base image ready: $OUTPUT (${SIZE_GB}G, $CODENAME, Node.js ${NODE_MAJOR}.x, $AGENT_COUNT agent users)"
echo "next: inject the guest agent with images/inject-agent.sh <agent-binary> $OUTPUT"

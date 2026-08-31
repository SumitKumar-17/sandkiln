#!/usr/bin/env bash
# One-time host bootstrap: everything sandkilnd needs to actually run,
# in the right order, from a completely fresh checkout. Replaces
# manually running install-firecracker.sh, building the guest agent,
# fetching/building an image, injecting the agent into it,
# create-tap-pool.sh, and grant-net-admin.sh yourself and keeping their
# paths/counts in sync by hand — the exact friction that made getting a
# working daemon up take a dozen commands instead of one.
#
# Safe to re-run: every step checks whether it's already done before
# doing it (matches this project's "development scripts must be safe to
# run repeatedly" rule in AGENTS.md section 12).
#
# Usage:
#   scripts/setup.sh [--production] [--tap-pool-size N] [--agent-count N] [--start]
#
# --production   Build the full universal rootfs image (Ubuntu + current
#                Node.js/Python + common tooling + multi-agent isolation,
#                ~6GiB, needs sudo and several GiB of free disk) via
#                images/build-universal-image.sh. Without this flag,
#                fetches the small ~300MiB Firecracker CI test image
#                instead — fast, proves the whole setup works end to end,
#                but missing ca-certificates/language runtimes and NOT
#                meant to back real workloads. You can upgrade to
#                --production later by re-running with it; it won't touch
#                a quick image already in place unless you remove it
#                first (SANDKILN_BASE_ROOTFS would need repointing either
#                way — this script tells you the exact new path).
# --tap-pool-size N   Default 32 (matches the daemon's own default, so
#                     you don't have to override SANDKILN_TAP_POOL_SIZE
#                     forever after this).
# --agent-count N     Multi-agent isolation users baked into a production
#                     image (--production only). Default 4.
# --start             Start the daemon immediately afterward via
#                     scripts/sandkilnd-ctl.sh start.
#
# Writes .env.sandkiln-setup at the repo root (gitignored, matches the
# project's existing .env* pattern) recording exactly what this run set
# up — scripts/sandkilnd-ctl.sh sources it automatically, and any
# SANDKILN_* var you've already exported yourself still wins over it.
# That's what makes `scripts/sandkilnd-ctl.sh start` with no env vars at
# all work correctly after running this once.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
IMAGES_DIR="$REPO_ROOT/images"
CORE_DIR="$REPO_ROOT/core"
TOOLS_DIR="${SANDKILN_TOOLS_DIR:-$HOME/sandkiln-tools}"
ENV_FILE="$REPO_ROOT/.env.sandkiln-setup"

PRODUCTION=0
TAP_POOL_SIZE=32
AGENT_COUNT=4
START_AFTER=0
while [ $# -gt 0 ]; do
  case "$1" in
    --production) PRODUCTION=1; shift ;;
    --tap-pool-size) TAP_POOL_SIZE="${2:?--tap-pool-size needs a number}"; shift 2 ;;
    --agent-count) AGENT_COUNT="${2:?--agent-count needs a number}"; shift 2 ;;
    --start) START_AFTER=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

FAIL=0
step() { echo; echo "==> $1"; }
ok()   { echo "    ok      - $1"; }
skip() { echo "    skip    - $1 (already done)"; }
bad()  { FAIL=1; echo "    FAIL    - $1" >&2; }

# [ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"  — see sandkilnd-ctl.sh's
# comment on why this matters in a non-interactive shell.
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

# ---------------------------------------------------------------------------
step "prerequisites"
# ---------------------------------------------------------------------------
if [ "$(uname -s)" != "Linux" ]; then
  bad "not running on Linux ($(uname -s)) — Firecracker requires Linux/KVM"
fi
if [ "$(uname -m)" != "x86_64" ]; then
  bad "host is $(uname -m), not x86_64 — this project's images/kernels target x86_64"
fi
if [ ! -e /dev/kvm ]; then
  bad "/dev/kvm does not exist — enable virtualization (and nested virtualization, if this is itself a VM)"
elif [ ! -r /dev/kvm ] || [ ! -w /dev/kvm ]; then
  bad "/dev/kvm exists but isn't read/write for $(whoami) — sudo usermod -aG kvm \$USER, then re-login"
else
  ok "/dev/kvm read/write for $(whoami)"
fi
command -v cargo >/dev/null 2>&1 || bad "cargo not found — install Rust via rustup first"
command -v sudo >/dev/null 2>&1 || bad "sudo not found — several steps below need it for one-time host setup"
if [ "$FAIL" -eq 1 ]; then
  echo
  echo "prerequisites not met — fix the FAIL items above before continuing" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
step "musl target for the guest agent"
# ---------------------------------------------------------------------------
if rustup target list --installed 2>/dev/null | grep -q x86_64-unknown-linux-musl; then
  skip "x86_64-unknown-linux-musl target"
else
  rustup target add x86_64-unknown-linux-musl
  ok "added x86_64-unknown-linux-musl target"
fi
if command -v musl-gcc >/dev/null 2>&1; then
  skip "musl-tools"
else
  echo "    musl-tools not found — installing (needs sudo)"
  if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get install -y musl-tools
  else
    bad "musl-gcc missing and this isn't a Debian/Ubuntu host — install musl-tools (or equivalent) yourself, then re-run"
  fi
fi

# ---------------------------------------------------------------------------
step "build the workspace and the guest agent"
# ---------------------------------------------------------------------------
(cd "$CORE_DIR" && cargo build --release --workspace) || { bad "workspace build failed"; exit 1; }
ok "workspace built"
(cd "$CORE_DIR" && cargo build --release -p sandkiln-guest-agent --target x86_64-unknown-linux-musl) \
  || { bad "guest agent build failed"; exit 1; }
AGENT_BIN="$CORE_DIR/target/x86_64-unknown-linux-musl/release/sandkiln-agent"
ok "guest agent built: $AGENT_BIN"

# ---------------------------------------------------------------------------
step "firecracker + jailer"
# ---------------------------------------------------------------------------
FC_BIN="$TOOLS_DIR/bin/firecracker"
if [ -x "$FC_BIN" ]; then
  skip "firecracker at $FC_BIN ($("$FC_BIN" --version 2>&1 | head -n1))"
else
  bash "$SCRIPT_DIR/install-firecracker.sh" "$TOOLS_DIR" || { bad "install-firecracker.sh failed"; exit 1; }
  ok "firecracker installed to $TOOLS_DIR/bin"
fi
KERNEL_PATH="$TOOLS_DIR/images/vmlinux-5.10.223"

# ---------------------------------------------------------------------------
step "base rootfs image"
# ---------------------------------------------------------------------------
if [ "$PRODUCTION" -eq 1 ]; then
  ROOTFS_PATH="$TOOLS_DIR/images/universal.ext4"
  if [ -f "$ROOTFS_PATH" ]; then
    skip "production image at $ROOTFS_PATH"
  else
    free_gib="$(df --output=avail -B1G "$TOOLS_DIR" 2>/dev/null | tail -n1 | tr -d ' ' || echo 0)"
    if [ "${free_gib:-0}" -lt 8 ]; then
      bad "only ${free_gib:-0}GiB free — building a production image needs ~8GiB headroom (6GiB image + package downloads). Free up space or drop --production."
      exit 1
    fi
    echo "    building production image (this takes a while, needs sudo)"
    sudo bash "$IMAGES_DIR/build-universal-image.sh" "$ROOTFS_PATH" 6 "" "$AGENT_COUNT" \
      || { bad "build-universal-image.sh failed"; exit 1; }
    ok "production image built: $ROOTFS_PATH"
  fi
else
  ROOTFS_PATH="$TOOLS_DIR/images/ubuntu-22.04.ext4"
  if [ -f "$ROOTFS_PATH" ] && [ -f "$KERNEL_PATH" ]; then
    skip "test kernel + rootfs at $TOOLS_DIR/images"
  else
    bash "$IMAGES_DIR/fetch-test-image.sh" "$TOOLS_DIR/images" || { bad "fetch-test-image.sh failed"; exit 1; }
    ok "test kernel + rootfs fetched to $TOOLS_DIR/images"
  fi
  echo "    NOTE: this is the small test image (~300MiB) — missing ca-certificates and language"
  echo "    runtimes, not meant for real workloads. Re-run with --production for a real one."
fi

# ---------------------------------------------------------------------------
step "inject the guest agent into the image"
# ---------------------------------------------------------------------------
# Cheap idempotency check: mount read-only and look for the binary we'd
# install, rather than unconditionally re-mounting+writing every run.
AGENT_ALREADY_BAKED=0
mnt="$(mktemp -d)"
if sudo mount -o loop,ro "$ROOTFS_PATH" "$mnt" 2>/dev/null; then
  [ -f "$mnt/usr/local/bin/sandkiln-agent" ] && AGENT_ALREADY_BAKED=1
  sudo umount "$mnt" 2>/dev/null || sudo umount -l "$mnt" 2>/dev/null || true
fi
rmdir "$mnt" 2>/dev/null || true

if [ "$AGENT_ALREADY_BAKED" -eq 1 ]; then
  skip "guest agent already baked into $ROOTFS_PATH"
else
  sudo bash "$IMAGES_DIR/inject-agent.sh" "$AGENT_BIN" "$ROOTFS_PATH" || { bad "inject-agent.sh failed"; exit 1; }
  ok "guest agent injected into $ROOTFS_PATH"
fi

# ---------------------------------------------------------------------------
step "tap device pool ($TAP_POOL_SIZE devices)"
# ---------------------------------------------------------------------------
existing=0
for ((i = 0; i < TAP_POOL_SIZE; i++)); do
  ip link show "sktap${i}" >/dev/null 2>&1 && existing=$((existing + 1))
done
if [ "$existing" -eq "$TAP_POOL_SIZE" ]; then
  skip "all $TAP_POOL_SIZE tap devices already present"
else
  sudo bash "$SCRIPT_DIR/create-tap-pool.sh" "$TAP_POOL_SIZE" "$(whoami)" sktap \
    || { bad "create-tap-pool.sh failed"; exit 1; }
  ok "tap pool ready: sktap0..sktap$((TAP_POOL_SIZE - 1))"
fi

# ---------------------------------------------------------------------------
step "CAP_NET_ADMIN on the daemon binary"
# ---------------------------------------------------------------------------
DAEMON_BIN="$CORE_DIR/target/release/sandkilnd"
if getcap "$DAEMON_BIN" 2>/dev/null | grep -q cap_net_admin; then
  skip "CAP_NET_ADMIN already granted"
else
  sudo bash "$SCRIPT_DIR/grant-net-admin.sh" "$DAEMON_BIN" || { bad "grant-net-admin.sh failed"; exit 1; }
  ok "CAP_NET_ADMIN granted (re-run this script, or just grant-net-admin.sh, after every rebuild)"
fi

# ---------------------------------------------------------------------------
step "writing $ENV_FILE"
# ---------------------------------------------------------------------------
cat > "$ENV_FILE" <<EOF
# Written by scripts/setup.sh — sourced automatically by
# scripts/sandkilnd-ctl.sh. Anything you've already exported yourself
# takes precedence (every line below only fills in if unset). Safe to
# edit by hand or delete; re-running setup.sh regenerates it.
: "\${SANDKILN_FIRECRACKER_BIN:=$FC_BIN}"
: "\${SANDKILN_KERNEL_PATH:=$KERNEL_PATH}"
: "\${SANDKILN_BASE_ROOTFS:=$ROOTFS_PATH}"
: "\${SANDKILN_TAP_POOL_PREFIX:=sktap}"
: "\${SANDKILN_TAP_POOL_SIZE:=$TAP_POOL_SIZE}"
: "\${SANDKILN_DRIVES_DIR:=$TOOLS_DIR/drives}"
export SANDKILN_FIRECRACKER_BIN SANDKILN_KERNEL_PATH SANDKILN_BASE_ROOTFS
export SANDKILN_TAP_POOL_PREFIX SANDKILN_TAP_POOL_SIZE SANDKILN_DRIVES_DIR
EOF
ok "wrote $ENV_FILE"

echo
echo "=== setup complete ==="
echo "firecracker:  $FC_BIN"
echo "kernel:       $KERNEL_PATH"
echo "rootfs:       $ROOTFS_PATH $([ "$PRODUCTION" -eq 0 ] && echo '(test image — see NOTE above)')"
echo "tap pool:     sktap0..sktap$((TAP_POOL_SIZE - 1))"
echo
echo "next: scripts/sandkilnd-ctl.sh start"
echo "(no env vars needed — $ENV_FILE covers it; set SANDKILN_AUTH_TOKEN"
echo " yourself before exposing this beyond localhost)"

if [ "$START_AFTER" -eq 1 ]; then
  echo
  exec bash "$SCRIPT_DIR/sandkilnd-ctl.sh" start --no-build
fi

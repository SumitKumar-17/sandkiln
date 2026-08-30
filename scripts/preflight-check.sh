#!/usr/bin/env bash
# Validates that a host is actually ready to run sandkilnd, before you
# spend time starting it and debugging why sandbox creation fails. Checks
# every prerequisite this project's own code and scripts assume: KVM
# access, the Firecracker binary, the kernel/rootfs images (including a
# check that the rootfs isn't still the stripped CI test image nothing
# should run production sandboxes from — see images/build-universal-image.sh),
# the tap pool, whether the daemon binary has CAP_NET_ADMIN, and whether
# the configured listen port is actually free.
#
# Reads the same SANDKILN_* env vars the daemon itself does (see
# core/crates/daemon/src/config.rs) and applies the same defaults, so
# running this with no arguments checks exactly what starting sandkilnd
# with no overrides would try to use.
#
# Most checks need no privilege. Pass --root-checks (run under `sudo -E`
# specifically — plain `sudo` resets HOME to /root, which breaks every
# `~/sandkiln-tools/...` default path this script and the daemon both
# use) to additionally loop-mount the rootfs image read-only and confirm
# the guest agent's systemd unit is actually baked in — the single most
# common way a "healthy-looking" setup still fails at sandbox creation.
#
# Usage: scripts/preflight-check.sh [--daemon-bin <path>] [--root-checks]
# Example: sudo -E scripts/preflight-check.sh --root-checks
# Exit code: 0 if every check passed, 1 if anything failed.

set -uo pipefail

DAEMON_BIN=""
ROOT_CHECKS=0
while [ $# -gt 0 ]; do
  case "$1" in
    --daemon-bin) DAEMON_BIN="${2:?--daemon-bin needs a path}"; shift 2 ;;
    --root-checks) ROOT_CHECKS=1; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

FAIL=0
WARN=0

ok()   { echo "  ok      - $1"; }
warn() { WARN=$((WARN + 1)); echo "  WARNING - $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  FAIL    - $1"; }
section() { echo; echo "=== $1 ==="; }

expand_home() {
  case "$1" in
    "~/"*) echo "$HOME/${1#\~/}" ;;
    *) echo "$1" ;;
  esac
}

# ---------------------------------------------------------------------------
# Same defaults as core/crates/daemon/src/config.rs::Config::from_env —
# keep these in sync with that file if its defaults ever change.
# ---------------------------------------------------------------------------
LISTEN_ADDR="${SANDKILN_LISTEN_ADDR:-127.0.0.1:7777}"
FIRECRACKER_BIN="$(expand_home "${SANDKILN_FIRECRACKER_BIN:-~/sandkiln-tools/bin/firecracker}")"
KERNEL_PATH="$(expand_home "${SANDKILN_KERNEL_PATH:-~/sandkiln-tools/images/vmlinux-5.10.223}")"
BASE_ROOTFS="$(expand_home "${SANDKILN_BASE_ROOTFS:-~/sandkiln-tools/images/ubuntu-22.04.ext4}")"
BRIDGE_NAME="${SANDKILN_BRIDGE_NAME:-sktapbr0}"
TAP_POOL_PREFIX="${SANDKILN_TAP_POOL_PREFIX:-sktap}"
TAP_POOL_SIZE="${SANDKILN_TAP_POOL_SIZE:-32}"
DRIVES_DIR="$(expand_home "${SANDKILN_DRIVES_DIR:-~/sandkiln-tools/drives}")"
AUTH_TOKEN="${SANDKILN_AUTH_TOKEN:-}"

echo "sandkiln preflight check"
echo "(reading the same SANDKILN_* env vars and defaults the daemon itself uses)"

# ---------------------------------------------------------------------------
section "host"
# ---------------------------------------------------------------------------
if [ "$(uname -s)" = "Linux" ]; then
  ok "running on Linux"
else
  bad "not running on Linux ($(uname -s)) — Firecracker requires Linux/KVM"
fi

if [ "$(uname -m)" = "x86_64" ]; then
  ok "x86_64 host"
else
  warn "host arch is $(uname -m), not x86_64 — this project's images/kernels are built for x86_64; aarch64 Firecracker exists but nothing here has been verified against it"
fi

if [ -e /dev/kvm ]; then
  if [ -r /dev/kvm ] && [ -w /dev/kvm ]; then
    ok "/dev/kvm present and read/write for $(whoami)"
  else
    bad "/dev/kvm exists but isn't read/write for $(whoami) — add your user to the kvm group: sudo usermod -aG kvm \$USER, then re-login"
  fi
else
  bad "/dev/kvm does not exist — KVM isn't available on this host (check virtualization is enabled, and if this is itself a VM, that nested virtualization is on)"
fi

# ---------------------------------------------------------------------------
section "firecracker"
# ---------------------------------------------------------------------------
if [ -x "$FIRECRACKER_BIN" ]; then
  ver="$("$FIRECRACKER_BIN" --version 2>&1 | head -n1 || true)"
  ok "firecracker binary present and executable: $FIRECRACKER_BIN ($ver)"
else
  bad "firecracker binary not found or not executable at $FIRECRACKER_BIN — run scripts/install-firecracker.sh, or set SANDKILN_FIRECRACKER_BIN"
fi

# ---------------------------------------------------------------------------
section "images"
# ---------------------------------------------------------------------------
if [ -f "$KERNEL_PATH" ]; then
  ok "kernel image present: $KERNEL_PATH"
else
  bad "kernel image not found at $KERNEL_PATH — run images/fetch-test-image.sh, or set SANDKILN_KERNEL_PATH"
fi

if [ -f "$BASE_ROOTFS" ]; then
  size_bytes="$(stat -c%s "$BASE_ROOTFS" 2>/dev/null || stat -f%z "$BASE_ROOTFS" 2>/dev/null || echo 0)"
  size_mib=$((size_bytes / 1024 / 1024))
  ok "base rootfs present: $BASE_ROOTFS (${size_mib} MiB)"

  # The unmodified Firecracker CI test image (images/fetch-test-image.sh's
  # output) is ~300MiB and missing ca-certificates/Node/Python — never
  # meant to back real sandboxes (see images/build-universal-image.sh's
  # header comment). It's also SANDKILN_BASE_ROOTFS's compiled-in default,
  # which is exactly the trap this check exists to catch: a fresh install
  # that never overrides the env var silently boots every sandbox from a
  # test image, not a production one.
  if [ "$size_mib" -lt 1024 ]; then
    warn "$BASE_ROOTFS is under 1GiB — this looks like the raw CI test image, not a built production rootfs (images/build-universal-image.sh's default output is several GiB). If that's intentional (e.g. you're just testing the setup mechanics), ignore this; otherwise build a real image and point SANDKILN_BASE_ROOTFS at it."
  fi

  if [ "$ROOT_CHECKS" -eq 1 ]; then
    if [ "$EUID" -ne 0 ]; then
      bad "--root-checks was passed but this isn't running as root — re-run with sudo to loop-mount and verify the guest agent is baked in"
    else
      mnt="$(mktemp -d)"
      if mount -o loop,ro "$BASE_ROOTFS" "$mnt" 2>/dev/null; then
        if [ -f "$mnt/usr/local/bin/sandkiln-agent" ] && [ -f "$mnt/etc/systemd/system/sandkiln-agent.service" ]; then
          ok "guest agent is baked into $BASE_ROOTFS (binary + systemd unit both present)"
        else
          bad "$BASE_ROOTFS has no guest agent baked in — sandboxes will boot but never respond to exec/read/write. Run images/inject-agent.sh <agent-binary> $BASE_ROOTFS"
        fi
        umount "$mnt" 2>/dev/null || umount -l "$mnt" 2>/dev/null || true
      else
        warn "could not loop-mount $BASE_ROOTFS to verify the guest agent — skipping that check"
      fi
      rmdir "$mnt" 2>/dev/null || true
    fi
  else
    warn "guest-agent-baked-in check skipped (needs root — re-run with sudo scripts/preflight-check.sh --root-checks)"
  fi
else
  bad "base rootfs not found at $BASE_ROOTFS — run images/fetch-test-image.sh for a quick test image, or images/build-universal-image.sh for a real one, then set SANDKILN_BASE_ROOTFS"
fi

# ---------------------------------------------------------------------------
section "networking"
# ---------------------------------------------------------------------------
if command -v ip >/dev/null 2>&1; then
  present=0
  missing=()
  for ((i = 0; i < TAP_POOL_SIZE; i++)); do
    name="${TAP_POOL_PREFIX}${i}"
    if ip link show "$name" >/dev/null 2>&1; then
      present=$((present + 1))
    else
      missing+=("$name")
    fi
  done
  if [ "$present" -eq "$TAP_POOL_SIZE" ]; then
    ok "all $TAP_POOL_SIZE tap devices present ($TAP_POOL_PREFIX*, matching SANDKILN_TAP_POOL_SIZE)"
  elif [ "$present" -eq 0 ]; then
    bad "no tap devices found matching ${TAP_POOL_PREFIX}0..${TAP_POOL_PREFIX}$((TAP_POOL_SIZE - 1)) — run: sudo scripts/create-tap-pool.sh $TAP_POOL_SIZE \$(whoami) $TAP_POOL_PREFIX"
  else
    bad "only $present of $TAP_POOL_SIZE expected tap devices exist — missing: ${missing[*]:0:5}$([ "${#missing[@]}" -gt 5 ] && echo " ...")"
  fi

  if ip link show "$BRIDGE_NAME" >/dev/null 2>&1; then
    ok "bridge $BRIDGE_NAME already exists (the daemon also creates this itself on startup if missing, so this is informational, not required beforehand)"
  fi
else
  bad "'ip' command not found — this host is missing iproute2"
fi

# ---------------------------------------------------------------------------
section "daemon binary capability"
# ---------------------------------------------------------------------------
if [ -n "$DAEMON_BIN" ]; then
  if [ -x "$DAEMON_BIN" ]; then
    if command -v getcap >/dev/null 2>&1; then
      cap_out="$(getcap "$DAEMON_BIN" 2>/dev/null || true)"
      if echo "$cap_out" | grep -q cap_net_admin; then
        ok "$DAEMON_BIN has CAP_NET_ADMIN: $cap_out"
      else
        warn "$DAEMON_BIN has no CAP_NET_ADMIN file capability. If you're running it under the provided systemd unit (scripts/sandkilnd.service), this is fine — it grants the capability per-start instead. Otherwise: sudo scripts/grant-net-admin.sh $DAEMON_BIN"
      fi
    else
      warn "'getcap' not found (libcap2-bin) — can't verify capability, skipping"
    fi
  else
    bad "--daemon-bin $DAEMON_BIN is not an executable file"
  fi
else
  warn "no --daemon-bin given — skipping the CAP_NET_ADMIN check (pass --daemon-bin <path-to-sandkilnd> to include it)"
fi

# ---------------------------------------------------------------------------
section "listen address"
# ---------------------------------------------------------------------------
host_part="${LISTEN_ADDR%:*}"
port_part="${LISTEN_ADDR##*:}"
if command -v ss >/dev/null 2>&1; then
  if ss -ltn "( sport = :$port_part )" 2>/dev/null | grep -q ":$port_part"; then
    bad "something is already listening on port $port_part ($LISTEN_ADDR) — stop it first, or set SANDKILN_LISTEN_ADDR to a free address"
  else
    ok "port $port_part is free ($LISTEN_ADDR)"
  fi
else
  warn "'ss' not found — can't check whether $LISTEN_ADDR is already in use"
fi

# ---------------------------------------------------------------------------
section "persistent storage"
# ---------------------------------------------------------------------------
parent="$(dirname "$DRIVES_DIR")"
if [ -d "$DRIVES_DIR" ] && [ -w "$DRIVES_DIR" ]; then
  ok "drives directory exists and is writable: $DRIVES_DIR"
elif [ -d "$parent" ] && [ -w "$parent" ]; then
  ok "drives directory doesn't exist yet but its parent is writable — the daemon creates it on startup: $DRIVES_DIR"
else
  bad "can't create the drives directory: $DRIVES_DIR (parent $parent doesn't exist or isn't writable)"
fi

# ---------------------------------------------------------------------------
section "authentication"
# ---------------------------------------------------------------------------
if [ -n "$AUTH_TOKEN" ]; then
  ok "SANDKILN_AUTH_TOKEN is set — the API will require it"
else
  warn "SANDKILN_AUTH_TOKEN is not set — the API will be completely unauthenticated. Fine for a quick local test, not for anything reachable beyond localhost."
fi

# ---------------------------------------------------------------------------
section "result"
# ---------------------------------------------------------------------------
echo "checks failed: $FAIL   warnings: $WARN"
if [ "$FAIL" -gt 0 ]; then
  echo "not ready — fix the FAIL items above before starting sandkilnd"
  exit 1
fi
echo "ready to start sandkilnd (review any WARNING items above first)"

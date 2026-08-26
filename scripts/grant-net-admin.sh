#!/usr/bin/env bash
# Grants the daemon binary CAP_NET_ADMIN so it can create tap devices and
# manage iptables rules without running as root. Needs sudo once, after
# each rebuild (setcap doesn't survive a binary being replaced).
#
# Usage: sudo scripts/grant-net-admin.sh <path-to-sandkilnd-binary>

set -euo pipefail

BIN="${1:?path to the sandkilnd binary required}"
setcap cap_net_admin+ep "$BIN"
getcap "$BIN"

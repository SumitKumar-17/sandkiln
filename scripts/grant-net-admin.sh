#!/usr/bin/env bash
# Grants the daemon binary CAP_NET_ADMIN so it can create tap devices and
# manage iptables rules without running as root. Needs sudo once, after
# each rebuild (setcap doesn't survive a binary being replaced).
#
# Usage: sudo scripts/grant-net-admin.sh <path-to-sandkilnd-binary>

set -euo pipefail

BIN="${1:?path to the sandkilnd binary required}"
# +i (inheritable) is required on top of +ep — the daemon raises this into
# its ambient set at startup so `ip`/`iptables` child processes inherit it
# too; a plain +ep grant doesn't propagate to children at all.
setcap cap_net_admin+eip "$BIN"
getcap "$BIN"

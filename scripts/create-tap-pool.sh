#!/usr/bin/env bash
# Creates a pool of persistent tap devices owned by a given user, for the
# daemon to lease from at runtime.
#
# Why a pool instead of creating taps on demand: the daemon runs
# unprivileged with CAP_NET_ADMIN granted via setcap + raised into its
# ambient set (see grant-net-admin.sh), which is enough for netlink
# operations (attach/detach to a bridge, bring up/down) — proven working.
# But creating a *new* tap device is a TUNSETIFF ioctl on /dev/net/tun, a
# different kernel check that ambient CAP_NET_ADMIN alone does not appear
# to satisfy in practice. Persistent tap devices sidestep this entirely:
# create them once as root, and the daemon only ever attaches/detaches
# existing devices, which is a plain netlink operation.
#
# Usage: sudo scripts/create-tap-pool.sh <count> <owner-user> [name-prefix]
# Example: sudo scripts/create-tap-pool.sh 32 t1000 sktap

set -euo pipefail

COUNT="${1:?number of tap devices required}"
OWNER="${2:?owning username required}"
PREFIX="${3:-sktap}"

for i in $(seq 0 $((COUNT - 1))); do
  name="${PREFIX}${i}"
  if ! ip link show "$name" &>/dev/null; then
    ip tuntap add "$name" mode tap user "$OWNER"
  fi
done

echo "$COUNT persistent tap devices ready: ${PREFIX}0..${PREFIX}$((COUNT - 1)), owned by $OWNER"

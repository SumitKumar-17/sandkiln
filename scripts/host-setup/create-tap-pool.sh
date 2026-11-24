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
# Usage: sudo scripts/host-setup/create-tap-pool.sh <count> <owner-user> [name-prefix] [group]
# Example: sudo scripts/host-setup/create-tap-pool.sh 32 t1000 sktap
#
# With [group] (the jailer's shared group, see prepare-jailer.sh) the taps are
# owned by that group INSTEAD of by <owner-user>: a jailed Firecracker runs under
# its own uid, and the kernel refuses a tap whose owner is a different uid.
# Members of the group may attach, which includes the daemon's own user.
# Example: sudo scripts/host-setup/create-tap-pool.sh 32 t1000 sktap sandkiln-jail

set -euo pipefail

COUNT="${1:?number of tap devices required}"
OWNER="${2:?owning username required}"
PREFIX="${3:-sktap}"
GROUP="${4:-}"
OWNED_BY="${GROUP:+group $GROUP}"
OWNED_BY="${OWNED_BY:-$OWNER}"

for i in $(seq 0 $((COUNT - 1))); do
  name="${PREFIX}${i}"
  if ! ip link show "$name" &>/dev/null; then
    if [ -n "$GROUP" ]; then
      ip tuntap add "$name" mode tap group "$GROUP"
    else
      ip tuntap add "$name" mode tap user "$OWNER"
    fi
  fi
done

echo "$COUNT persistent tap devices ready: ${PREFIX}0..${PREFIX}$((COUNT - 1)), owned by $OWNED_BY"

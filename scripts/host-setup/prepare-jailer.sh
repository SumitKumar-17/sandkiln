#!/usr/bin/env bash
# One-time root steps that let the unprivileged daemon use Firecracker's
# jailer. Idempotent. Everything here is something the daemon deliberately
# cannot do for itself; see docs-site/self-hosting/jailer.mdx for why each
# step exists and docs/design-notes/security-hardening.md for the design.
#
#   1. a shared jail group, with the daemon's user in it. Every jailed VM runs
#      under its own uid but this one gid; the group is how the daemon reaches
#      a jail's sockets, tears it down, and (via the tap pool) how a jailed
#      Firecracker may attach to a tap.
#   2. the jailer binary setuid root (it needs chroot, setuid, cgroups).
#   3. the parent cgroup directory owned by the daemon's user, so the daemon
#      can remove each VM's cgroup after it exits (jailer never does).
#   4. the chroot base directory, owned by the daemon's user.
#
# The group membership applies to NEW login sessions. Start the daemon from
# one (log in again, or `sg <group> -c '<command>'`), or the startup
# self-check will refuse to start and say so.
#
# Usage: sudo scripts/host-setup/prepare-jailer.sh <daemon-user> [options]
#   --gid N                  shared jail group id        (default 600000, = jailer_gid)
#   --group-name NAME        its name                    (default sandkiln-jail)
#   --jailer-bin PATH        (default ~<daemon-user>/sandkiln-tools/bin/jailer)
#   --supervisor-name NAME   file name of the supervisor binary; it names the
#                            parent cgroup (default sandkiln-firecracker-supervisor)
#   --chroot-base PATH       (default ~<daemon-user>/sandkiln-tools/jail)
#
# Then create the tap pool for jailed use (a pool owned by a group, not a user):
#   sudo scripts/host-setup/create-tap-pool.sh <count> <daemon-user> <prefix> <group-name>

set -euo pipefail

[ "$(id -u)" -eq 0 ] || { echo "must run as root: sudo $0 <daemon-user> [options]" >&2; exit 1; }

DAEMON_USER="${1:?daemon user required}"
shift
id "$DAEMON_USER" >/dev/null 2>&1 || { echo "no such user: $DAEMON_USER" >&2; exit 1; }
USER_HOME="$(getent passwd "$DAEMON_USER" | cut -d: -f6)"

GID=600000
GROUP_NAME=sandkiln-jail
JAILER_BIN="$USER_HOME/sandkiln-tools/bin/jailer"
SUPERVISOR_NAME=sandkiln-firecracker-supervisor
CHROOT_BASE="$USER_HOME/sandkiln-tools/jail"
while [ $# -gt 0 ]; do
  case "$1" in
    --gid) GID="${2:?--gid needs a number}"; shift 2 ;;
    --group-name) GROUP_NAME="${2:?--group-name needs a name}"; shift 2 ;;
    --jailer-bin) JAILER_BIN="${2:?--jailer-bin needs a path}"; shift 2 ;;
    --supervisor-name) SUPERVISOR_NAME="${2:?--supervisor-name needs a name}"; shift 2 ;;
    --chroot-base) CHROOT_BASE="${2:?--chroot-base needs a path}"; shift 2 ;;
    *) echo "unknown option: $1" >&2; exit 1 ;;
  esac
done

case "$SUPERVISOR_NAME" in
  *firecracker*) ;;
  *) echo "--supervisor-name must contain \"firecracker\" (jailer refuses other exec file names)" >&2; exit 1 ;;
esac
[ -f "$JAILER_BIN" ] || { echo "jailer binary not found: $JAILER_BIN (run scripts/host-setup/install-firecracker.sh)" >&2; exit 1; }
[ -f /sys/fs/cgroup/cgroup.controllers ] \
  || { echo "cgroup v2 is not mounted at /sys/fs/cgroup; the jailer needs it" >&2; exit 1; }

# 1. group + membership
if getent group "$GROUP_NAME" >/dev/null; then
  existing="$(getent group "$GROUP_NAME" | cut -d: -f3)"
  [ "$existing" = "$GID" ] || { echo "group $GROUP_NAME exists with gid $existing, not $GID; pass --gid $existing or pick another name" >&2; exit 1; }
elif getent group "$GID" >/dev/null; then
  echo "gid $GID is already taken by group $(getent group "$GID" | cut -d: -f1); pick another with --gid" >&2
  exit 1
else
  groupadd --gid "$GID" "$GROUP_NAME"
  echo "created group $GROUP_NAME ($GID)"
fi
if id -nG "$DAEMON_USER" | tr ' ' '\n' | grep -qx "$GROUP_NAME"; then
  echo "$DAEMON_USER is already in $GROUP_NAME"
else
  usermod -aG "$GROUP_NAME" "$DAEMON_USER"
  echo "added $DAEMON_USER to $GROUP_NAME (takes effect in new login sessions)"
fi

# 2. setuid-root jailer
chown root:root "$JAILER_BIN"
chmod u+s "$JAILER_BIN"
echo "jailer is setuid root: $(ls -l "$JAILER_BIN")"

# 3. parent cgroup owned by the daemon's user. The jailer names it after the
# exec file. Controllers must be enabled for its children.
CGROUP="/sys/fs/cgroup/$SUPERVISOR_NAME"
mkdir -p "$CGROUP"
chown "$DAEMON_USER" "$CGROUP"
echo "cgroup $CGROUP owned by $DAEMON_USER"

# 4. chroot base
mkdir -p "$CHROOT_BASE"
chown "$DAEMON_USER" "$CHROOT_BASE"
echo "chroot base $CHROOT_BASE owned by $DAEMON_USER"

echo "done. Start the daemon from a session that has the group ($GROUP_NAME); jailer_gid must be $GID."

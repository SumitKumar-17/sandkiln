#!/usr/bin/env bash
# Installs sandkilnd as a systemd service from scripts/sandkilnd.service.template,
# substituting real paths for the daemon binary, an unprivileged user/group
# to run it as, and an environment file for its SANDKILN_* configuration.
# Needs sudo (writes to /etc/systemd/system and reloads the daemon).
#
# Deliberately does NOT `setcap` the binary — the installed unit grants
# CAP_NET_ADMIN via systemd's own AmbientCapabilities= at every start
# instead, which is what makes this survive a rebuild with no extra step
# (see the template's comment and SELF_HOSTING.md).
#
# Usage:
#   sudo scripts/install-systemd-service.sh <path-to-sandkilnd> [user] [env-file]
# Example:
#   sudo scripts/install-systemd-service.sh /home/t1000/sandkiln/core/target/release/sandkilnd t1000 /etc/sandkiln/sandkilnd.env

set -euo pipefail

DAEMON_BIN="${1:?path to the built sandkilnd binary required}"
RUN_USER="${2:-${SUDO_USER:-$(whoami)}}"
ENV_FILE="${3:-/etc/sandkiln/sandkilnd.env}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="$SCRIPT_DIR/sandkilnd.service.template"
UNIT_PATH="/etc/systemd/system/sandkilnd.service"

if [[ $EUID -ne 0 ]]; then
  echo "must run as root (sudo) — writes to /etc/systemd/system" >&2
  exit 1
fi

if [[ ! -x "$DAEMON_BIN" ]]; then
  echo "not an executable file: $DAEMON_BIN" >&2
  exit 1
fi
DAEMON_BIN="$(readlink -f "$DAEMON_BIN")"

if ! id "$RUN_USER" >/dev/null 2>&1; then
  echo "user '$RUN_USER' does not exist on this host — create it first, or pass an existing username" >&2
  exit 1
fi
RUN_GROUP="$(id -gn "$RUN_USER")"

if [[ ! -f "$TEMPLATE" ]]; then
  echo "missing template: $TEMPLATE" >&2
  exit 1
fi

mkdir -p "$(dirname "$ENV_FILE")"
if [[ ! -f "$ENV_FILE" ]]; then
  cat > "$ENV_FILE" <<EOF
# sandkilnd environment — see SELF_HOSTING.md's configuration table for
# every SANDKILN_* variable this daemon reads. Uncomment and set at least
# SANDKILN_AUTH_TOKEN before exposing this beyond localhost.
#SANDKILN_AUTH_TOKEN=
#SANDKILN_LISTEN_ADDR=127.0.0.1:7777
EOF
  chmod 600 "$ENV_FILE"
  echo "created $ENV_FILE (edit it before starting the service, especially SANDKILN_AUTH_TOKEN)"
else
  echo "$ENV_FILE already exists, leaving it as-is"
fi

sed \
  -e "s#@SANDKILN_BIN_DIR@#$SCRIPT_DIR#g" \
  -e "s#@SANDKILN_DAEMON_BIN@#$DAEMON_BIN#g" \
  -e "s#@SANDKILN_USER@#$RUN_USER#g" \
  -e "s#@SANDKILN_GROUP@#$RUN_GROUP#g" \
  -e "s#@SANDKILN_ENV_FILE@#$ENV_FILE#g" \
  "$TEMPLATE" > "$UNIT_PATH"

chmod 644 "$UNIT_PATH"
systemctl daemon-reload

echo "installed $UNIT_PATH"
echo "  binary:  $DAEMON_BIN"
echo "  user:    $RUN_USER:$RUN_GROUP"
echo "  env:     $ENV_FILE"
echo
echo "next: sudo systemctl enable --now sandkilnd"
echo "logs: journalctl -u sandkilnd -f"

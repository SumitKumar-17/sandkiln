#!/usr/bin/env bash
# Installs and starts a dnsmasq forwarder for guest VM DNS, as a systemd
# service so it survives host reboots.
#
# Why a forwarder: some networks block a VM (or the host, on raw UDP/53)
# from reaching public resolvers directly, while the host's own
# systemd-resolved stub (127.0.0.53) works. dnsmasq forwards guest queries
# to that stub, and is the natural place to add domain allowlisting later.
#
# Why --bind-dynamic: the bridge (and therefore its gateway IP) is created
# by sandkilnd at startup, not by host setup. Binding strictly to an
# address that doesn't exist yet fails; --bind-dynamic binds as soon as
# the address appears, so start order (and reboots) don't matter.
#
# Usage: sudo scripts/host-setup/start-dns-proxy.sh <tap-gateway-ip>
# Example: sudo scripts/host-setup/start-dns-proxy.sh 172.16.0.1

set -euo pipefail

LISTEN_IP="${1:?tap gateway ip required, e.g. 172.16.0.1}"
[ "$(id -u)" -eq 0 ] || { echo "must run as root: sudo $0 $LISTEN_IP" >&2; exit 1; }
DNSMASQ="$(command -v dnsmasq)" || { echo "dnsmasq not installed (apt-get install dnsmasq-base)" >&2; exit 1; }
UNIT=/etc/systemd/system/sandkiln-dnsmasq.service

# Stop any hand-started instance from an older version of this script.
pkill -x dnsmasq 2>/dev/null || true

cat > "$UNIT" <<UNIT_EOF
[Unit]
Description=sandkiln guest DNS forwarder
After=network.target systemd-resolved.service

[Service]
ExecStart=$DNSMASQ --keep-in-foreground --no-resolv --server=127.0.0.53 \\
  --listen-address=$LISTEN_IP --bind-dynamic --no-hosts --no-poll \\
  --log-facility=/var/log/sandkiln-dnsmasq.log
Restart=on-failure
RestartSec=2

[Install]
WantedBy=multi-user.target
UNIT_EOF

systemctl daemon-reload
systemctl enable sandkiln-dnsmasq.service >/dev/null 2>&1
systemctl restart sandkiln-dnsmasq.service

# Readiness: the unit must be active. It cannot be probed on $LISTEN_IP:53
# yet if the bridge hasn't been created (that happens when sandkilnd starts).
for _ in 1 2 3 4 5; do
  systemctl is-active --quiet sandkiln-dnsmasq.service && { echo "dns proxy service active (binds $LISTEN_IP:53 once the bridge exists)"; exit 0; }
  sleep 0.5
done
journalctl -u sandkiln-dnsmasq.service -n 10 --no-pager >&2
exit 1

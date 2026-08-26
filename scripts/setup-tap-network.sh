#!/usr/bin/env bash
# Creates one tap device for a Firecracker microVM and wires up NAT so the
# guest gets outbound internet. Point-to-point, no bridge — matches
# Firecracker's own quickstart networking model. Needs sudo.
#
# Usage: sudo scripts/setup-tap-network.sh <tap-name> <host-ip>/<prefix> <uplink-iface>
# Example: sudo scripts/setup-tap-network.sh fc-tap0 172.16.0.1/24 enp0s31f6

set -euo pipefail

TAP="${1:?tap device name required}"
HOST_CIDR="${2:?host-side ip/prefix required, e.g. 172.16.0.1/24}"
UPLINK="${3:?uplink interface required, e.g. enp0s31f6}"
OWNER="${SUDO_USER:-$(whoami)}"

if ! ip link show "$TAP" &>/dev/null; then
  ip tuntap add "$TAP" mode tap user "$OWNER"
fi
ip addr replace "$HOST_CIDR" dev "$TAP"
ip link set "$TAP" up

sysctl -w net.ipv4.ip_forward=1 >/dev/null

# Idempotent: only add the MASQUERADE/FORWARD rules if not already present.
iptables -t nat -C POSTROUTING -o "$UPLINK" -j MASQUERADE 2>/dev/null || \
  iptables -t nat -A POSTROUTING -o "$UPLINK" -j MASQUERADE
iptables -C FORWARD -i "$TAP" -o "$UPLINK" -j ACCEPT 2>/dev/null || \
  iptables -A FORWARD -i "$TAP" -o "$UPLINK" -j ACCEPT
iptables -C FORWARD -i "$UPLINK" -o "$TAP" -m state --state RELATED,ESTABLISHED -j ACCEPT 2>/dev/null || \
  iptables -A FORWARD -i "$UPLINK" -o "$TAP" -m state --state RELATED,ESTABLISHED -j ACCEPT

# DNS: redirect the guest's queries to the host's own local resolver
# (systemd-resolved's stub at 127.0.0.53), instead of relying on the guest
# reaching a public resolver directly — some networks block or intercept
# outbound port 53 in ways that only work correctly from the host itself.
sysctl -w "net.ipv4.conf.${TAP}.route_localnet=1" >/dev/null
iptables -t nat -C PREROUTING -i "$TAP" -p udp --dport 53 -j DNAT --to-destination 127.0.0.53:53 2>/dev/null || \
  iptables -t nat -A PREROUTING -i "$TAP" -p udp --dport 53 -j DNAT --to-destination 127.0.0.53:53
iptables -t nat -C PREROUTING -i "$TAP" -p tcp --dport 53 -j DNAT --to-destination 127.0.0.53:53 2>/dev/null || \
  iptables -t nat -A PREROUTING -i "$TAP" -p tcp --dport 53 -j DNAT --to-destination 127.0.0.53:53

echo "tap device $TAP ready, host side $HOST_CIDR, NAT via $UPLINK, guest DNS -> host resolver"

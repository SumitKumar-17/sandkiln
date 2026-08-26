#!/usr/bin/env bash
# Runs a small dnsmasq forwarder bound to the tap gateway IP, so guest VMs
# get working DNS without needing direct access to a public resolver.
#
# Why this exists: some networks don't let a VM (or even the host, on raw
# UDP/53) reach 8.8.8.8/1.1.1.1 directly, but name resolution through the
# host's own resolver (systemd-resolved's 127.0.0.53 stub) works fine.
# dnsmasq here just forwards guest queries to that already-working
# resolver, as a normal host-local client — this also gives us a natural
# place to add domain allowlisting later (Phase 7).
#
# Usage: sudo scripts/start-dns-proxy.sh <tap-gateway-ip>
# Example: sudo scripts/start-dns-proxy.sh 172.16.0.1

set -euo pipefail

LISTEN_IP="${1:?tap gateway ip required, e.g. 172.16.0.1}"

pkill -x dnsmasq 2>/dev/null || true
sleep 1

nohup dnsmasq --no-daemon --no-resolv --server=127.0.0.53 \
  --listen-address="$LISTEN_IP" --bind-interfaces --no-hosts --no-poll \
  --log-facility=/tmp/sandkiln-dnsmasq.log \
  > /tmp/sandkiln-dnsmasq.out 2>&1 &
disown

sleep 1
ss -lnup | grep "$LISTEN_IP:53" && echo "dns proxy listening on $LISTEN_IP:53"

#!/usr/bin/env bash
# Drive N sequential cold `POST /sandboxes` creates against a running
# daemon and print one line per create, so the daemon's own debug-level
# phase breakdown (see `routes_sandbox`'s "cold create setup phases" /
# "cold create complete" and `sandkiln-vmm`'s "vm boot phase breakdown"
# events) can be lined up against real client-observed latency.
#
# This is a profiling aid, not a benchmark harness — it deliberately does
# one create at a time and deletes each sandbox before the next, so every
# sample measures an uncontended cold create rather than throughput.
#
# Usage:
#   scripts/dev-tools/profile-cold-create.sh [iterations] [base-url]
#
# Run the daemon with RUST_LOG=sandkiln_daemon=debug,sandkiln_vmm=debug to
# get the phase events this is meant to be read alongside.

set -uo pipefail

ITERATIONS="${1:-20}"
BASE_URL="${2:-http://127.0.0.1:7777}"
AUTH=()
[ -n "${SANDKILN_AUTH_TOKEN:-}" ] && AUTH=(-H "Authorization: Bearer $SANDKILN_AUTH_TOKEN")

for i in $(seq 1 "$ITERATIONS"); do
  response="$(curl -s -w '\n%{time_total}' --max-time 60 "${AUTH[@]}" \
    -X POST "$BASE_URL/sandboxes" -H 'Content-Type: application/json' -d '{}')"
  body="$(printf '%s' "$response" | head -n1)"
  seconds="$(printf '%s' "$response" | tail -n1)"
  id="$(printf '%s' "$body" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')"

  if [ -z "$id" ]; then
    echo "create $i FAILED: $body" >&2
    continue
  fi
  # Millisecond-resolution, to match the units every phase event uses.
  echo "create $i id=$id total_ms=$(awk -v s="$seconds" 'BEGIN { printf "%.1f", s * 1000 }')"

  # `keep=false` destroys outright instead of the default
  # snapshot-then-stop — a snapshot per iteration would add several
  # hundred ms of unrelated work between samples.
  curl -s -o /dev/null --max-time 60 "${AUTH[@]}" -X DELETE "$BASE_URL/sandboxes/$id?keep=false"
done

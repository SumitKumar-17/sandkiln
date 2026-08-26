#!/usr/bin/env bash
# Concurrent load test against a running sandkilnd daemon: N workers each
# run create -> exec -> delete cycles against the HTTP API and report
# min/max/mean/p95 latency per phase. This is deliberately hand-rolled with
# curl -w timing + sort/awk rather than hyperfine: each cycle is stateful
# (the exec/delete calls need the sandbox id the create call returned) and
# concurrent (many cycles in flight at once), and hyperfine is built around
# repeating one fixed, stateless command sequentially — neither fits.
#
# Usage: scripts/load-test.sh [concurrency] [iterations-per-worker] [base-url]
# Example: scripts/load-test.sh 10 20 http://127.0.0.1:7777
#
# base-url can also be set via SANDKILN_LOAD_TEST_URL; the positional arg
# wins if both are given.

set -uo pipefail

CONCURRENCY="${1:-10}"
ITERATIONS="${2:-20}"
BASE_URL="${3:-${SANDKILN_LOAD_TEST_URL:-http://127.0.0.1:7777}}"
BASE_URL="${BASE_URL%/}"

if ! [[ "$CONCURRENCY" =~ ^[0-9]+$ ]] || [ "$CONCURRENCY" -lt 1 ]; then
  echo "concurrency must be a positive integer, got: $CONCURRENCY" >&2
  exit 1
fi
if ! [[ "$ITERATIONS" =~ ^[0-9]+$ ]] || [ "$ITERATIONS" -lt 1 ]; then
  echo "iterations must be a positive integer, got: $ITERATIONS" >&2
  exit 1
fi

if ! curl -sf -o /dev/null "$BASE_URL/healthz"; then
  echo "sandkilnd not reachable at $BASE_URL/healthz — is it running?" >&2
  exit 1
fi

WORKDIR="$(mktemp -d /tmp/sandkiln-load-test-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

echo "sandkiln load test: $CONCURRENCY workers x $ITERATIONS cycles against $BASE_URL"

extract_id() {
  if command -v jq >/dev/null 2>&1; then
    jq -r '.id // empty'
  else
    grep -o '"id"[[:space:]]*:[[:space:]]*"[^"]*"' | sed -E 's/.*:"([^"]*)"/\1/'
  fi
}

# Issues one HTTP request, writes the response body to $3, and prints
# "<http_code> <time_total>" — used to both time and status-check every
# call without a second round trip.
do_request() {
  local method="$1" url="$2" out="$3" data="${4:-}"
  if [ -n "$data" ]; then
    curl -s -o "$out" -w '%{http_code} %{time_total}' -X "$method" "$url" \
      -H 'Content-Type: application/json' -d "$data"
  else
    curl -s -o "$out" -w '%{http_code} %{time_total}' -X "$method" "$url"
  fi
}

run_worker() {
  local worker_id="$1"
  local body="$WORKDIR/w${worker_id}.body"
  local create_times="$WORKDIR/w${worker_id}.create.times"
  local exec_times="$WORKDIR/w${worker_id}.exec.times"
  local delete_times="$WORKDIR/w${worker_id}.delete.times"
  local cycle_times="$WORKDIR/w${worker_id}.cycle.times"
  local errors="$WORKDIR/w${worker_id}.errors"
  : > "$create_times"; : > "$exec_times"; : > "$delete_times"; : > "$cycle_times"; : > "$errors"

  local i
  for ((i = 0; i < ITERATIONS; i++)); do
    local cycle_start
    cycle_start="$(date +%s.%N)"

    local result status time id
    result="$(do_request POST "$BASE_URL/sandboxes" "$body")"
    status="${result%% *}"; time="${result##* }"
    if [ "$status" != "200" ] && [ "$status" != "201" ]; then
      echo "create: http $status" >> "$errors"
      continue
    fi
    echo "$time" >> "$create_times"
    id="$(extract_id < "$body")"
    if [ -z "$id" ]; then
      echo "create: no id in response body" >> "$errors"
      continue
    fi

    result="$(do_request POST "$BASE_URL/sandboxes/$id/exec" "$body" '{"command":"true","args":[]}')"
    status="${result%% *}"; time="${result##* }"
    if [ "$status" != "200" ]; then
      echo "exec: http $status" >> "$errors"
    else
      echo "$time" >> "$exec_times"
    fi

    result="$(do_request DELETE "$BASE_URL/sandboxes/$id" "$body")"
    status="${result%% *}"; time="${result##* }"
    if [ "$status" != "200" ] && [ "$status" != "204" ]; then
      echo "delete: http $status" >> "$errors"
    else
      echo "$time" >> "$delete_times"
    fi

    awk -v start="$cycle_start" -v end="$(date +%s.%N)" 'BEGIN { printf "%.6f\n", end - start }' >> "$cycle_times"
  done
}

START="$(date +%s.%N)"
pids=()
for ((w = 0; w < CONCURRENCY; w++)); do
  run_worker "$w" &
  pids+=("$!")
done
for pid in "${pids[@]}"; do
  wait "$pid"
done
END="$(date +%s.%N)"

cat "$WORKDIR"/w*.create.times > "$WORKDIR/all.create.times" 2>/dev/null || true
cat "$WORKDIR"/w*.exec.times > "$WORKDIR/all.exec.times" 2>/dev/null || true
cat "$WORKDIR"/w*.delete.times > "$WORKDIR/all.delete.times" 2>/dev/null || true
cat "$WORKDIR"/w*.cycle.times > "$WORKDIR/all.cycle.times" 2>/dev/null || true
cat "$WORKDIR"/w*.errors > "$WORKDIR/all.errors" 2>/dev/null || true

stats() {
  local label="$1" file="$2"
  local n
  n="$(wc -l < "$file" | tr -d ' ')"
  if [ "$n" -eq 0 ]; then
    printf '%-8s n=0 (no successful samples)\n' "$label"
    return
  fi
  sort -n "$file" > "$file.sorted"
  local min max mean p95_idx p95
  min="$(head -n1 "$file.sorted")"
  max="$(tail -n1 "$file.sorted")"
  mean="$(awk '{s+=$1} END {printf "%.4f", s/NR}' "$file.sorted")"
  p95_idx="$(awk -v n="$n" 'BEGIN { i = int(n * 0.95); if (i < 1) i = 1; if (i > n) i = n; print i }')"
  p95="$(sed -n "${p95_idx}p" "$file.sorted")"
  printf '%-8s n=%-6s min=%-8ss max=%-8ss mean=%-8ss p95=%-8ss\n' "$label" "$n" "$min" "$max" "$mean" "$p95"
}

total_cycles=$((CONCURRENCY * ITERATIONS))
error_count="$(wc -l < "$WORKDIR/all.errors" | tr -d ' ')"
wall_seconds="$(awk -v s="$START" -v e="$END" 'BEGIN { printf "%.3f", e - s }')"
throughput="$(awk -v n="$total_cycles" -v s="$wall_seconds" 'BEGIN { if (s > 0) printf "%.2f", n / s; else print "n/a" }')"

echo
echo "=== results ==="
echo "wall time:      ${wall_seconds}s"
echo "cycles:         $total_cycles requested, $error_count errors"
echo "throughput:     ${throughput} cycles/sec"
echo
stats "create" "$WORKDIR/all.create.times"
stats "exec" "$WORKDIR/all.exec.times"
stats "delete" "$WORKDIR/all.delete.times"
stats "cycle" "$WORKDIR/all.cycle.times"

if [ "$error_count" -gt 0 ]; then
  echo
  echo "=== errors (first 20) ==="
  head -n 20 "$WORKDIR/all.errors"
fi

[ "$error_count" -eq 0 ]

#!/usr/bin/env bash
# One command for "how fast is a cold create right now, broken down by
# phase, and did that change since last time" -- replaces eyeballing
# scripts/dev-tools/profile-cold-create.sh's output against hand-enabled
# debug logs with a real before/after diff of the daemon's own
# /metrics histograms (create_phase_duration_ms{phase=...},
# boot_duration_ms -- see core/crates/daemon/src/metrics.rs).
#
# Since Prometheus histograms are cumulative for the process's lifetime,
# this snapshots /metrics before and after driving N sequential cold
# creates and diffs sum/count per phase -- that isolates *this run's*
# contribution even against a daemon that's already served other
# traffic, no restart needed. No special RUST_LOG needed either: these
# are metrics, not debug-level tracing.
#
# Each run's phase-mean breakdown is saved as
# scripts/bench-results/<UTC-timestamp>.json (gitignored -- these numbers
# are dev-box-specific and directional, not meant to be committed) and
# compared against scripts/bench-results/latest.json if one exists, so a
# real regression or win shows up as a clear percentage the next time
# this runs, not something that has to be noticed by chance the way the
# CoW-rootfs mistake earlier in this project's history was.
#
# This deliberately only covers the phases already promoted to /metrics
# (rootfs_clone, network_lease, setup, total, boot) -- the finer-grained
# breakdown inside Vm::boot itself (socket wait, each Firecracker PUT,
# InstanceStart) stays debug-level tracing on purpose (see metrics.rs's
# own doc comment on why) and is out of scope here; reach for
# `scripts/dev-tools/profile-cold-create.sh` with RUST_LOG=...=debug for
# that finer level instead.
#
# Usage: scripts/bench-report.sh [iterations] [base-url]
# Example: scripts/bench-report.sh 20 http://127.0.0.1:7777

set -uo pipefail

ITERATIONS="${1:-20}"
BASE_URL="${2:-${SANDKILN_BENCH_REPORT_URL:-http://127.0.0.1:7777}}"
BASE_URL="${BASE_URL%/}"
AUTH=()
[ -n "${SANDKILN_AUTH_TOKEN:-}" ] && AUTH=(-H "Authorization: Bearer $SANDKILN_AUTH_TOKEN")

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RESULTS_DIR="$SCRIPT_DIR/bench-results"
mkdir -p "$RESULTS_DIR"

if ! [[ "$ITERATIONS" =~ ^[0-9]+$ ]] || [ "$ITERATIONS" -lt 1 ]; then
  echo "iterations must be a positive integer, got: $ITERATIONS" >&2
  exit 1
fi
if ! curl -sf -o /dev/null "${AUTH[@]}" "$BASE_URL/healthz"; then
  echo "sandkilnd not reachable at $BASE_URL/healthz -- is it running?" >&2
  exit 1
fi

# All four `create_phase_duration_ms` phases plus the separately-named
# `boot_duration_ms` -- see metrics.rs's own doc comment on why boot
# isn't folded into that family.
PHASES=(rootfs_clone network_lease setup total)

metrics_snapshot() {
  curl -s "${AUTH[@]}" "$BASE_URL/metrics"
}

# sum_for <metrics-text> <metric-name> [phase-label]
sum_for() {
  local text="$1" name="$2" phase="${3:-}"
  if [ -n "$phase" ]; then
    printf '%s\n' "$text" | grep -F "${name}_sum{phase=\"${phase}\"}" | awk '{print $2}'
  else
    printf '%s\n' "$text" | grep -F "${name}_sum " | awk '{print $2}'
  fi
}

count_for() {
  local text="$1" name="$2" phase="${3:-}"
  if [ -n "$phase" ]; then
    printf '%s\n' "$text" | grep -F "${name}_count{phase=\"${phase}\"}" | awk '{print $2}'
  else
    printf '%s\n' "$text" | grep -F "${name}_count " | awk '{print $2}'
  fi
}

echo "sandkiln bench-report: $ITERATIONS sequential cold creates against $BASE_URL"
BEFORE="$(metrics_snapshot)"

for i in $(seq 1 "$ITERATIONS"); do
  body="$(curl -s "${AUTH[@]}" -X POST "$BASE_URL/sandboxes" -H 'Content-Type: application/json' -d '{}')"
  id="$(printf '%s' "$body" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')"
  if [ -z "$id" ]; then
    echo "create $i FAILED: $body" >&2
    continue
  fi
  # keep=false: a full destroy, not the default snapshot-then-stop --
  # a snapshot per iteration would add several hundred ms of unrelated
  # work between samples, corrupting the very thing being measured.
  curl -s -o /dev/null "${AUTH[@]}" -X DELETE "$BASE_URL/sandboxes/$id?keep=false"
  printf '.'
done
echo

AFTER="$(metrics_snapshot)"

TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"
GIT_SHA="$(git -C "$SCRIPT_DIR/.." rev-parse --short HEAD 2>/dev/null || echo unknown)"

# name:phase pairs -- boot_duration_ms has no phase label of its own.
METRIC_SPECS=("boot_duration_ms:" "create_phase_duration_ms:rootfs_clone" "create_phase_duration_ms:network_lease" "create_phase_duration_ms:setup" "create_phase_duration_ms:total")

declare -a ROW_LABELS ROW_N ROW_MEAN
JSON_PHASES=""

for spec in "${METRIC_SPECS[@]}"; do
  name="${spec%%:*}"
  phase="${spec#*:}"
  label="$name"; [ -n "$phase" ] && label="$phase"

  before_sum="$(sum_for "$BEFORE" "$name" "$phase")"; before_count="$(count_for "$BEFORE" "$name" "$phase")"
  after_sum="$(sum_for "$AFTER" "$name" "$phase")"; after_count="$(count_for "$AFTER" "$name" "$phase")"
  before_sum="${before_sum:-0}"; before_count="${before_count:-0}"
  after_sum="${after_sum:-0}"; after_count="${after_count:-0}"

  delta_sum="$(awk -v a="$after_sum" -v b="$before_sum" 'BEGIN { printf "%.6f", a - b }')"
  delta_count="$(awk -v a="$after_count" -v b="$before_count" 'BEGIN { print a - b }')"

  if [ "$delta_count" -le 0 ]; then
    mean="n/a"
  else
    mean="$(awk -v s="$delta_sum" -v n="$delta_count" 'BEGIN { printf "%.3f", s / n }')"
  fi

  ROW_LABELS+=("$label"); ROW_N+=("$delta_count"); ROW_MEAN+=("$mean")
  JSON_PHASES="${JSON_PHASES}{\"phase\":\"${label}\",\"n\":${delta_count},\"mean_ms\":$( [ "$mean" = "n/a" ] && echo null || echo "$mean" )},"
done
JSON_PHASES="[${JSON_PHASES%,}]"

RESULT_FILE="$RESULTS_DIR/${TIMESTAMP}.json"
cat > "$RESULT_FILE" <<EOF
{"timestamp":"$TIMESTAMP","git_sha":"$GIT_SHA","iterations":$ITERATIONS,"base_url":"$BASE_URL","phases":$JSON_PHASES}
EOF

echo
echo "=== this run ($TIMESTAMP, $GIT_SHA) ==="
printf '%-16s %-6s %s\n' "phase" "n" "mean_ms"
for idx in "${!ROW_LABELS[@]}"; do
  printf '%-16s %-6s %s\n' "${ROW_LABELS[$idx]}" "${ROW_N[$idx]}" "${ROW_MEAN[$idx]}"
done

LATEST="$RESULTS_DIR/latest.json"
if [ -f "$LATEST" ] && command -v jq >/dev/null 2>&1; then
  echo
  echo "=== vs. last run ($(jq -r '.timestamp' "$LATEST") $(jq -r '.git_sha' "$LATEST")) ==="
  printf '%-16s %-10s %-10s %s\n' "phase" "was" "now" "change"
  for idx in "${!ROW_LABELS[@]}"; do
    label="${ROW_LABELS[$idx]}"
    now="${ROW_MEAN[$idx]}"
    was="$(jq -r --arg p "$label" '.phases[] | select(.phase == $p) | .mean_ms // "n/a"' "$LATEST")"
    if [ "$was" = "n/a" ] || [ "$was" = "null" ] || [ -z "$was" ] || [ "$now" = "n/a" ]; then
      printf '%-16s %-10s %-10s %s\n' "$label" "${was:-n/a}" "$now" "-"
      continue
    fi
    pct="$(awk -v was="$was" -v now="$now" 'BEGIN { if (was == 0) { print "n/a" } else { printf "%.1f", (now - was) / was * 100 } }')"
    flag="~"
    if awk -v p="$pct" 'BEGIN { exit !(p+0 >= 10) }' 2>/dev/null; then flag="regressed"; fi
    if awk -v p="$pct" 'BEGIN { exit !(p+0 <= -10) }' 2>/dev/null; then flag="improved"; fi
    printf '%-16s %-10s %-10s %s%% (%s)\n' "$label" "$was" "$now" "$pct" "$flag"
  done
elif [ -f "$LATEST" ]; then
  echo
  echo "(install jq to see a comparison against the last run -- $LATEST exists but this script can't parse it without jq)"
fi

cp "$RESULT_FILE" "$LATEST"
echo
echo "saved: $RESULT_FILE (and updated $LATEST)"

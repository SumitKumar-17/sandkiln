#!/usr/bin/env bash
# One command for "how fast is sandkiln right now, broken down by phase,
# did that change since last time, and is the pre-warmed-pool win real" --
# replaces eyeballing scripts/dev-tools/profile-cold-create.sh's output
# against hand-enabled debug logs with a real before/after diff of the
# daemon's own /metrics histograms (create_phase_duration_ms{phase=...},
# boot_duration_ms -- see core/crates/daemon/src/metrics.rs), plus two
# client-side timings /metrics can't see at all (see below) and,
# optionally, the criterion micro-benchmark suite.
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
# Also reports two client-side timings, deliberately not sourced from
# /metrics: create() returning 200 means Firecracker's InstanceStart
# succeeded, not that the guest agent is listening yet, and none of the
# daemon-side phases above capture the gap until it actually answers.
#   - `first_exec_client`: a cold sandbox's first real exec, timed the
#     way a real caller experiences it. Found live: this absorbed
#     ~420-460ms in Vm::call's own retry loop on this dev box, while
#     every exec after the first on that same sandbox measured ~3-5ms.
#   - `first_exec_resumed`: the same measurement against a sandbox
#     resumed from a snapshot of an already-warmed one (agent confirmed
#     live before the snapshot was taken) -- measured ~4-18ms, a
#     ~25-100x difference `first_exec_client` alone would never surface,
#     since a cold-only run has nothing to compare it against.
# See ROADMAP.md's Benchmarking section for the full write-up of both.
#
# Usage: scripts/bench-report.sh [iterations] [base-url] [--with-criterion] [--markdown]
# Example: scripts/bench-report.sh 20 http://127.0.0.1:7777 --with-criterion --markdown
#
# --with-criterion also runs `cargo bench -p sandkiln-vmm --bench
# vm_lifecycle` (cold_boot/exec_roundtrip/resume_from_snapshot/
# snapshot_take) and folds its numbers into the same report -- off by
# default since it takes noticeably longer and needs real Firecracker
# assets (SANDKILN_BENCH_FIRECRACKER_BIN/_KERNEL_PATH/_ROOTFS_PATH, same
# defaults as `scripts/dev.sh bench`).
# --markdown prints a ready-to-paste table in ROADMAP.md's own style, in
# addition to the plain report -- specifically to close the gap that let
# a real re-measurement land in the website/docs without ever being
# copied into ROADMAP.md's own Benchmarking section: one command now
# produces the exact text meant to go in both places, instead of numbers
# transcribed by hand into some but not all of them.

set -uo pipefail

WITH_CRITERION=0
MARKDOWN=0
POSITIONAL=()
for arg in "$@"; do
  case "$arg" in
    --with-criterion) WITH_CRITERION=1 ;;
    --markdown) MARKDOWN=1 ;;
    *) POSITIONAL+=("$arg") ;;
  esac
done

ITERATIONS="${POSITIONAL[0]:-20}"
BASE_URL="${POSITIONAL[1]:-${SANDKILN_BENCH_REPORT_URL:-http://127.0.0.1:7777}}"
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

# create_sandbox -> prints the new id on stdout, or nothing on failure.
create_sandbox() {
  local body id
  body="$(curl -s "${AUTH[@]}" -X POST "$BASE_URL/sandboxes" -H 'Content-Type: application/json' -d '{}')"
  id="$(printf '%s' "$body" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')"
  [ -n "$id" ] && printf '%s' "$id"
}

# timed_exec <sandbox-id> -> prints elapsed milliseconds on stdout.
timed_exec() {
  local id="$1" start end
  start="$(date +%s%N)"
  curl -s -o /dev/null "${AUTH[@]}" -X POST "$BASE_URL/sandboxes/$id/exec" -H 'Content-Type: application/json' -d '{"command":"true","args":[]}'
  end="$(date +%s%N)"
  echo $(( (end - start) / 1000000 ))
}

destroy_sandbox() {
  curl -s -o /dev/null "${AUTH[@]}" -X DELETE "$BASE_URL/sandboxes/$1?keep=false"
}

echo "sandkiln bench-report: $ITERATIONS sequential cold creates against $BASE_URL"
BEFORE="$(metrics_snapshot)"

FIRST_EXEC_TOTAL_MS=0
FIRST_EXEC_N=0

for i in $(seq 1 "$ITERATIONS"); do
  id="$(create_sandbox)"
  if [ -z "$id" ]; then
    echo "create $i FAILED" >&2
    continue
  fi
  exec_ms="$(timed_exec "$id")"
  FIRST_EXEC_TOTAL_MS=$((FIRST_EXEC_TOTAL_MS + exec_ms))
  FIRST_EXEC_N=$((FIRST_EXEC_N + 1))
  # keep=false: a full destroy, not the default snapshot-then-stop -- a
  # snapshot per iteration would add several hundred ms of unrelated
  # work between samples, corrupting the very thing being measured.
  destroy_sandbox "$id"
  printf '.'
done
echo

AFTER="$(metrics_snapshot)"

# A separate, much shorter loop against forked sandboxes -- see this
# file's own header comment for why this needs to exist at all. One
# warm-up create+exec+snapshot establishes a checkpoint whose agent is
# confirmed live before it's frozen; every fork after that measures
# first-exec against a fresh instance of that same checkpoint. Fork
# (not resume) deliberately: fork does not consume the snapshot it came
# from, so the same snap_id can be forked again immediately after each
# forked instance is destroyed -- no history-chaining needed, and no
# risk of "resume" retiring the checkpoint into a differently-shaped
# record each time (retired-checkpoint history is backed by a HashMap
# server-side, so it has no reliable chronological order to chain
# through by hand). Capped at 10 regardless of $ITERATIONS -- a forked
# sandbox's first exec has far less variance than a cold one, and this
# needs Firecracker/KVM real resume work per sample rather than just an
# HTTP round trip.
RESUME_ITERATIONS=10
[ "$ITERATIONS" -lt "$RESUME_ITERATIONS" ] && RESUME_ITERATIONS="$ITERATIONS"
FIRST_EXEC_RESUMED_TOTAL_MS=0
FIRST_EXEC_RESUMED_N=0

warm_id="$(create_sandbox)"
if [ -z "$warm_id" ]; then
  echo "warm-up create for resume measurement FAILED -- skipping first_exec_resumed" >&2
else
  curl -s -o /dev/null "${AUTH[@]}" -X POST "$BASE_URL/sandboxes/$warm_id/exec" -H 'Content-Type: application/json' -d '{"command":"true","args":[]}'
  snap_body="$(curl -s "${AUTH[@]}" -X POST "$BASE_URL/sandboxes/$warm_id/snapshot")"
  snap_id="$(printf '%s' "$snap_body" | sed -n 's/.*"snapshot_id":"\([^"]*\)".*/\1/p')"
  if [ -z "$snap_id" ]; then
    echo "snapshot for resume measurement FAILED: $snap_body" >&2
  else
    for i in $(seq 1 "$RESUME_ITERATIONS"); do
      fork_body="$(curl -s "${AUTH[@]}" -X POST "$BASE_URL/snapshots/$snap_id/fork")"
      rid="$(printf '%s' "$fork_body" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')"
      if [ -z "$rid" ]; then
        echo "fork $i FAILED: $fork_body" >&2
        continue
      fi
      exec_ms="$(timed_exec "$rid")"
      FIRST_EXEC_RESUMED_TOTAL_MS=$((FIRST_EXEC_RESUMED_TOTAL_MS + exec_ms))
      FIRST_EXEC_RESUMED_N=$((FIRST_EXEC_RESUMED_N + 1))
      # A fork's own network lease/rootfs are borrowed from the
      # snapshot it came from, not owned outright -- destroying it
      # (keep=false) releases them back to that same snapshot, which is
      # exactly what makes forking snap_id again on the next loop
      # iteration safe rather than a conflict.
      destroy_sandbox "$rid"
      printf 'r'
    done
    echo
  fi
fi

FIRST_EXEC_MEAN="n/a"
[ "$FIRST_EXEC_N" -gt 0 ] && FIRST_EXEC_MEAN="$(awk -v s="$FIRST_EXEC_TOTAL_MS" -v n="$FIRST_EXEC_N" 'BEGIN { printf "%.3f", s / n }')"
FIRST_EXEC_RESUMED_MEAN="n/a"
[ "$FIRST_EXEC_RESUMED_N" -gt 0 ] && FIRST_EXEC_RESUMED_MEAN="$(awk -v s="$FIRST_EXEC_RESUMED_TOTAL_MS" -v n="$FIRST_EXEC_RESUMED_N" 'BEGIN { printf "%.3f", s / n }')"

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

# Client-side, not from /metrics -- see this file's own header comment
# for why these two aren't just more daemon-side phases.
ROW_LABELS+=("first_exec_client"); ROW_N+=("$FIRST_EXEC_N"); ROW_MEAN+=("$FIRST_EXEC_MEAN")
JSON_PHASES="${JSON_PHASES}{\"phase\":\"first_exec_client\",\"n\":${FIRST_EXEC_N},\"mean_ms\":$( [ "$FIRST_EXEC_MEAN" = "n/a" ] && echo null || echo "$FIRST_EXEC_MEAN" )},"
ROW_LABELS+=("first_exec_resumed"); ROW_N+=("$FIRST_EXEC_RESUMED_N"); ROW_MEAN+=("$FIRST_EXEC_RESUMED_MEAN")
JSON_PHASES="${JSON_PHASES}{\"phase\":\"first_exec_resumed\",\"n\":${FIRST_EXEC_RESUMED_N},\"mean_ms\":$( [ "$FIRST_EXEC_RESUMED_MEAN" = "n/a" ] && echo null || echo "$FIRST_EXEC_RESUMED_MEAN" )},"

# --with-criterion: cargo bench's own criterion suite, folded into the
# same report. Separate from everything above -- these measure one
# operation in isolation (no HTTP, no daemon) rather than through the
# real API, which is exactly why both this script's own phases AND the
# criterion suite are each worth having; neither replaces the other.
CRITERION_ROWS=""
if [ "$WITH_CRITERION" -eq 1 ]; then
  echo
  echo "running criterion suite (cargo bench -p sandkiln-vmm --bench vm_lifecycle)..."
  CORE_DIR="$SCRIPT_DIR/../core"
  export SANDKILN_BENCH_FIRECRACKER_BIN="${SANDKILN_BENCH_FIRECRACKER_BIN:-$HOME/sandkiln-tools/bin/firecracker}"
  export SANDKILN_BENCH_KERNEL_PATH="${SANDKILN_BENCH_KERNEL_PATH:-$HOME/sandkiln-tools/images/vmlinux-5.10.223}"
  export SANDKILN_BENCH_ROOTFS_PATH="${SANDKILN_BENCH_ROOTFS_PATH:-$HOME/sandkiln-tools/images/sandkiln-base.ext4}"
  if ! (cd "$CORE_DIR" && cargo bench -p sandkiln-vmm --bench vm_lifecycle >/tmp/sandkiln-bench-report-criterion.log 2>&1); then
    echo "criterion suite failed -- see /tmp/sandkiln-bench-report-criterion.log; continuing without it" >&2
  else
    for bench in cold_boot exec_roundtrip resume_from_snapshot snapshot_take; do
      estimates="$CORE_DIR/target/criterion/vm_lifecycle/$bench/new/estimates.json"
      [ -f "$estimates" ] || continue
      if command -v jq >/dev/null 2>&1; then
        mean_ms="$(jq -r '.mean.point_estimate / 1000000' "$estimates")"
        lo_ms="$(jq -r '.mean.confidence_interval.lower_bound / 1000000' "$estimates")"
        hi_ms="$(jq -r '.mean.confidence_interval.upper_bound / 1000000' "$estimates")"
      else
        mean_ms="$(python3 -c "import json;d=json.load(open('$estimates'));print(d['mean']['point_estimate']/1e6)" 2>/dev/null)"
        lo_ms="$(python3 -c "import json;d=json.load(open('$estimates'));print(d['mean']['confidence_interval']['lower_bound']/1e6)" 2>/dev/null)"
        hi_ms="$(python3 -c "import json;d=json.load(open('$estimates'));print(d['mean']['confidence_interval']['upper_bound']/1e6)" 2>/dev/null)"
      fi
      [ -z "$mean_ms" ] && continue
      mean_fmt="$(awk -v v="$mean_ms" 'BEGIN{printf "%.3f", v}')"
      lo_fmt="$(awk -v v="$lo_ms" 'BEGIN{printf "%.2f", v}')"
      hi_fmt="$(awk -v v="$hi_ms" 'BEGIN{printf "%.2f", v}')"
      ROW_LABELS+=("criterion:$bench"); ROW_N+=("-"); ROW_MEAN+=("$mean_fmt (${lo_fmt}-${hi_fmt})")
      CRITERION_ROWS="${CRITERION_ROWS}{\"bench\":\"${bench}\",\"mean_ms\":${mean_fmt},\"ci_low_ms\":${lo_fmt},\"ci_high_ms\":${hi_fmt}},"
    done
  fi
fi
CRITERION_ROWS="[${CRITERION_ROWS%,}]"

JSON_PHASES="[${JSON_PHASES%,}]"

RESULT_FILE="$RESULTS_DIR/${TIMESTAMP}.json"
cat > "$RESULT_FILE" <<EOF
{"timestamp":"$TIMESTAMP","git_sha":"$GIT_SHA","iterations":$ITERATIONS,"base_url":"$BASE_URL","phases":$JSON_PHASES,"criterion":$CRITERION_ROWS}
EOF

echo
echo "=== this run ($TIMESTAMP, $GIT_SHA) ==="
printf '%-20s %-6s %s\n' "phase" "n" "mean_ms"
for idx in "${!ROW_LABELS[@]}"; do
  printf '%-20s %-6s %s\n' "${ROW_LABELS[$idx]}" "${ROW_N[$idx]}" "${ROW_MEAN[$idx]}"
done

LATEST="$RESULTS_DIR/latest.json"
if [ -f "$LATEST" ] && command -v jq >/dev/null 2>&1; then
  echo
  echo "=== vs. last run ($(jq -r '.timestamp' "$LATEST") $(jq -r '.git_sha' "$LATEST")) ==="
  printf '%-20s %-10s %-10s %s\n' "phase" "was" "now" "change"
  for idx in "${!ROW_LABELS[@]}"; do
    label="${ROW_LABELS[$idx]}"
    now="${ROW_MEAN[$idx]}"
    case "$label" in criterion:*) continue ;; esac
    was="$(jq -r --arg p "$label" '.phases[] | select(.phase == $p) | .mean_ms // "n/a"' "$LATEST")"
    if [ "$was" = "n/a" ] || [ "$was" = "null" ] || [ -z "$was" ] || [ "$now" = "n/a" ]; then
      printf '%-20s %-10s %-10s %s\n' "$label" "${was:-n/a}" "$now" "-"
      continue
    fi
    pct="$(awk -v was="$was" -v now="$now" 'BEGIN { if (was == 0) { print "n/a" } else { printf "%.1f", (now - was) / was * 100 } }')"
    flag="~"
    if awk -v p="$pct" 'BEGIN { exit !(p+0 >= 10) }' 2>/dev/null; then flag="regressed"; fi
    if awk -v p="$pct" 'BEGIN { exit !(p+0 <= -10) }' 2>/dev/null; then flag="improved"; fi
    printf '%-20s %-10s %-10s %s%% (%s)\n' "$label" "$was" "$now" "$pct" "$flag"
  done
elif [ -f "$LATEST" ]; then
  echo
  echo "(install jq to see a comparison against the last run -- $LATEST exists but this script can't parse it without jq)"
fi

if [ "$MARKDOWN" -eq 1 ]; then
  echo
  echo "=== markdown (paste into ROADMAP.md / the website) ==="
  echo
  echo "| phase | n | mean |"
  echo "| --- | --- | --- |"
  for idx in "${!ROW_LABELS[@]}"; do
    printf '| %s | %s | %sms |\n' "${ROW_LABELS[$idx]}" "${ROW_N[$idx]}" "${ROW_MEAN[$idx]}"
  done
fi

cp "$RESULT_FILE" "$LATEST"
echo
echo "saved: $RESULT_FILE (and updated $LATEST)"

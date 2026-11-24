#!/usr/bin/env bash
# Where does one snapshot take go? Creates a sandbox per run, takes its
# snapshot and prints the daemon's own per-step timings
# (snapshot_take_phase_duration_ms{phase=...} in /metrics, diffed around the
# one request) next to the HTTP time a client sees, then the median of each
# column. Everything it creates is deleted again.
#
# Usage: scripts/snapshot-profile.sh [--mem MIB] [--dirty MIB] [--runs N]
#                                    [--settle SECS] [base-url]
#
#   --mem MIB      guest memory of the sandbox (default 512)
#   --dirty MIB    before the snapshot, write this much random data to the
#                  guest's tmpfs so that many MiB of memory are dirty (default 0:
#                  a freshly booted guest)
#   --runs N       sandboxes to take a snapshot of (default 10)
#   --settle SECS  wait this long after the first command before snapshotting
#                  (default 0.7). A guest younger than 500 ms makes `pause`
#                  sleep until it is that old (see MIN_GUEST_AGE_BEFORE_PAUSE),
#                  which would show up as the `pause` column; the default
#                  keeps that out so the other columns are the real work. Pass
#                  0 to measure a snapshot right after create.
#   base-url       daemon (default $SANDKILN_DAEMON_URL or http://127.0.0.1:7777)
#
# The box should be quiet (the daemon idle, no other disk work) and the
# daemon's default warm pool off (`default_warm_count = 0`), otherwise
# background refills take snapshots into the same histograms. Needs curl and jq.
set -euo pipefail

MEM=512 DIRTY=0 RUNS=10 SETTLE=0.7 BASE="${SANDKILN_DAEMON_URL:-http://127.0.0.1:7777}"
while [ $# -gt 0 ]; do
  case "$1" in
    --mem) MEM="${2:?--mem needs a value}"; shift 2 ;;
    --dirty) DIRTY="${2:?--dirty needs a value}"; shift 2 ;;
    --runs) RUNS="${2:?--runs needs a value}"; shift 2 ;;
    --settle) SETTLE="${2:?--settle needs a value}"; shift 2 ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "unknown option $1 (see --help)" >&2; exit 2 ;;
    *) BASE="$1"; shift ;;
  esac
done
for tool in curl jq awk; do
  command -v "$tool" >/dev/null || { echo "snapshot-profile: $tool is required" >&2; exit 1; }
done
AUTH=()
[ -n "${SANDKILN_AUTH_TOKEN:-}" ] && AUTH=(-H "Authorization: Bearer $SANDKILN_AUTH_TOKEN")
curl -sf "${AUTH[@]}" "$BASE/healthz" >/dev/null || { echo "snapshot-profile: no daemon answering at $BASE" >&2; exit 1; }

live="$(curl -s "${AUTH[@]}" "$BASE/status" | jq -r '.counts.sandboxes')"
[ "$live" = 0 ] || echo "note: $live live sandbox(es) on the daemon, timings will be noisier" >&2

PHASES=(pause create force_stop release persist total)
api() { curl -s "${AUTH[@]}" -H 'Content-Type: application/json' "$@"; }

phase_sums() { # one "phase value" line per phase, from the histogram sums
  api "$BASE/metrics" | sed -n 's/^snapshot_take_phase_duration_ms_sum{phase="\([a-z_]*\)"} /\1 /p'
}

SANDBOX="" SNAPSHOT=""
cleanup() {
  [ -n "$SANDBOX" ] && api -o /dev/null -X DELETE "$BASE/sandboxes/$SANDBOX?keep=false" || true
  [ -n "$SNAPSHOT" ] && api -o /dev/null -X DELETE "$BASE/snapshots/$SNAPSHOT" || true
}
trap cleanup EXIT

printf '%-4s %8s' run http_ms
printf ' %10s' "${PHASES[@]}"
echo
rows=()
for run in $(seq "$RUNS"); do
  SANDBOX="$(api -X POST "$BASE/sandboxes" -d "{\"mem_size_mib\":$MEM,\"tags\":{\"suite\":\"snapshot-profile\"}}" | jq -er '.id')"
  api -o /dev/null -X POST "$BASE/sandboxes/$SANDBOX/exec" -d '{"command":"true"}'
  if [ "$DIRTY" -gt 0 ]; then
    body="$(jq -nc --arg c "head -c ${DIRTY}M /dev/urandom > /dev/shm/dirty" '{command:"sh",args:["-c",$c]}')"
    api -o /dev/null -X POST "$BASE/sandboxes/$SANDBOX/exec" -d "$body"
  fi
  sleep "$SETTLE"
  before="$(phase_sums)"
  http="$(api -o "${TMPDIR:-/tmp}/snapshot-profile.$$" -w '%{time_total}' -X POST "$BASE/sandboxes/$SANDBOX/snapshot")"
  SNAPSHOT="$(jq -er '.snapshot_id' < "${TMPDIR:-/tmp}/snapshot-profile.$$")"
  SANDBOX=""
  after="$(phase_sums)"
  row="$(awk -v http="$http" -v order="${PHASES[*]}" '
    NR == FNR { b[$1] = $2; next }
    { a[$1] = $2 }
    END {
      printf "%.0f", http * 1000
      n = split(order, p, " ")
      for (i = 1; i <= n; i++) printf " %.0f", a[p[i]] - b[p[i]]
    }' <(printf '%s\n' "$before") <(printf '%s\n' "$after"))"
  rows+=("$row")
  printf '%-4s' "$run"
  awk '{ printf " %8s", $1; for (i = 2; i <= NF; i++) printf " %10s", $i; print "" }' <<<"$row"
  api -o /dev/null -X DELETE "$BASE/snapshots/$SNAPSHOT"
  SNAPSHOT=""
done
rm -f "${TMPDIR:-/tmp}/snapshot-profile.$$"

printf '%-4s' med
printf '%s\n' "${rows[@]}" | awk '
  { for (i = 1; i <= NF; i++) v[i, NR] = $i; n = NR; cols = NF }
  END {
    for (c = 1; c <= cols; c++) {
      k = 0
      for (r = 1; r <= n; r++) col[++k] = v[c, r]
      for (i = 1; i <= k; i++) for (j = i + 1; j <= k; j++) if (col[j] + 0 < col[i] + 0) { t = col[i]; col[i] = col[j]; col[j] = t }
      m = (k % 2) ? col[(k + 1) / 2] : (col[k / 2] + col[k / 2 + 1]) / 2
      printf (c == 1 ? " %8s" : " %10s"), m
    }
    print ""
  }'
echo "(ms; mem=${MEM} MiB, dirty=${DIRTY} MiB, settle=${SETTLE}s, ${RUNS} runs)"

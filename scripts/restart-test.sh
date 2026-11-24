#!/usr/bin/env bash
# Verifies orderly shutdown against a real daemon: live sandboxes are
# snapshotted on SIGTERM, no Firecracker process is left behind, and the
# snapshots are still there (and resumable) after a restart.
#
# It restarts the daemon, so it is not part of integration-test.sh, whose
# topics share one long-running daemon.
#
# SANDKILN_INTEGRATION_TEST_IMAGE=<registered-image-id> boots the sandboxes from
# that image (for example the init-mode image) instead of the daemon default.
# Firecracker processes are tracked by the pids of the daemon's own children,
# so other daemons on the same box do not change the result.
#
# Usage: scripts/restart-test.sh [base-url]
set -uo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE="${1:-http://127.0.0.1:7777}"
CTL="$SCRIPT_DIR/sandkilnd-ctl.sh"
PID_FILE="${SANDKILN_CTL_PID_FILE:-/tmp/sandkiln-daemon.pid}"
FAIL=0
ok()  { echo "  ok   - $1"; }
bad() { echo "  FAIL - $1"; FAIL=$((FAIL + 1)); }
json() { python3 -c "import sys,json;d=json.load(sys.stdin);print($1)"; }
AUTH=()
[ -n "${SANDKILN_AUTH_TOKEN:-}" ] && AUTH=(-H "Authorization: Bearer $SANDKILN_AUTH_TOKEN")
api() { curl -s "${AUTH[@]}" "$@"; }

run="restart-test-$$"
IMAGE_JSON=""
[ -n "${SANDKILN_INTEGRATION_TEST_IMAGE:-}" ] && IMAGE_JSON=",\"image_id\":\"$SANDKILN_INTEGRATION_TEST_IMAGE\""
echo "=== orderly shutdown ==="
"$CTL" status >/dev/null 2>&1 || { echo "daemon not running: start it first (scripts/sandkilnd-ctl.sh start)"; exit 2; }

for i in 1 2 3; do
  api -X POST "$BASE/sandboxes" -H 'content-type: application/json' -d "{\"name\":\"$run-$i\"$IMAGE_JSON}" -o /dev/null
done
pid="$(cat "$PID_FILE")"
children="$(pgrep -P "$pid" -x firecracker)"
before="$(echo "$children" | grep -c .)"
kill -TERM "$pid"
for _ in $(seq 1 120); do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
kill -0 "$pid" 2>/dev/null && bad "daemon still running 60s after SIGTERM" || ok "daemon exited after SIGTERM"
[ "$before" -ge 3 ] && ok "the daemon had $before Firecracker children before the signal" || bad "expected 3 Firecracker children before the signal, saw $before"
alive=0; for c in $children; do kill -0 "$c" 2>/dev/null && alive=$((alive + 1)); done
[ "$alive" -eq 0 ] && ok "every Firecracker child exited with the daemon" || bad "$alive Firecracker child(ren) outlived the daemon"

"$CTL" start --no-build >/dev/null 2>&1 || { bad "daemon failed to restart"; exit 1; }
names="$(api "$BASE/snapshots" | json '" ".join(sorted(s.get("name") or "" for s in d["snapshots"]))')"
for i in 1 2 3; do
  case " $names " in *" $run-$i "*) ok "sandbox $run-$i survived the restart as a snapshot" ;; *) bad "sandbox $run-$i was lost" ;; esac
done

sid="$(api "$BASE/snapshots" | json "next((s['id'] for s in d['snapshots'] if s.get('name')=='$run-1'), '')")"
rid="$(api -X POST "$BASE/snapshots/$sid/resume" | json 'd.get("id","")')"
out="$(api -m 20 -X POST "$BASE/sandboxes/$rid/exec" -H 'content-type: application/json' -d '{"command":"echo","args":["alive"]}')"
case "$out" in *alive*) ok "a snapshot taken at shutdown resumes and runs commands" ;; *) bad "resumed sandbox did not respond: $out" ;; esac

[ -n "$rid" ] && api -X DELETE "$BASE/sandboxes/$rid?keep=false" -o /dev/null
for i in 1 2 3; do
  s="$(api "$BASE/snapshots" | json "next((s['id'] for s in d['snapshots'] if s.get('name')=='$run-$i'), '')")"
  [ -n "$s" ] && api -X DELETE "$BASE/snapshots/$s" -o /dev/null
done
echo; [ "$FAIL" -eq 0 ] && echo "restart test: passed" || echo "restart test: $FAIL failed"
exit "$FAIL"

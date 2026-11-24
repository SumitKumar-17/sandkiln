#!/usr/bin/env bash
# Offline test of run.sh against a stand-in `kiln` that records its calls.
# It proves the control flow (exit-code propagation, cleanup, keep-on-failure,
# input validation), not the daemon: that is verified live, see
# examples/github-actions/README.md.
#
# Usage: test/test-run.sh

set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
RUN="$HERE/../run.sh"
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin" "$WORK/ws"
echo hi >"$WORK/ws/file.txt"

cat >"$WORK/bin/kiln" <<'STUB'
#!/usr/bin/env bash
echo "$*" >>"$FAKE_LOG"
case "$1 $2" in
  "sandbox create") echo fake-sandbox-id ;;
  "sandbox exec-stream") exit "${FAKE_STREAM_EXIT:-0}" ;;
  "sandbox read") printf '%s\n' "${FAKE_REPORTED:-0}" ;;
  "sandbox rm") echo "destroyed" ;;
esac
STUB
chmod +x "$WORK/bin/kiln"

failures=0
# scenario name, expected run exit, expected rm calls, then env assignments
scenario() {
  local name=$1 want_rc=$2 want_rm=$3 rc=0 rm_calls; shift 3
  export FAKE_LOG="$WORK/log"; : >"$FAKE_LOG"
  export GITHUB_OUTPUT="$WORK/out"; : >"$GITHUB_OUTPUT"
  export RUNNER_TEMP="$WORK/tmp-$name" GITHUB_WORKSPACE="$WORK/ws" GITHUB_RUN_ID=$name
  mkdir -p "$RUNNER_TEMP"
  env PATH="$WORK/bin:$PATH" "INPUT_DAEMON-URL=https://daemon.example" INPUT_COMMAND=true \
    "INPUT_KEEP-ON-FAILURE=false" "$@" bash "$RUN" run >/dev/null 2>&1 || rc=$?
  env PATH="$WORK/bin:$PATH" "INPUT_DAEMON-URL=https://daemon.example" \
    "INPUT_KEEP-ON-FAILURE=$(grep -q 'KEEP' <<<"$*" && echo true || echo false)" bash "$RUN" cleanup >/dev/null 2>&1 || true
  rm_calls=$(grep -c '^sandbox rm' "$FAKE_LOG" || true)
  if [ "$rc" = "$want_rc" ] && [ "$rm_calls" = "$want_rm" ]; then
    echo "ok   $name (exit $rc, rm calls $rm_calls)"
  else
    echo "FAIL $name: exit $rc (want $want_rc), rm calls $rm_calls (want $want_rm)"; failures=$((failures + 1))
  fi
}

scenario success 0 1
scenario command-fails 3 1 FAKE_STREAM_EXIT=3 FAKE_REPORTED=3
scenario stream-drops-reports-zero 125 1 FAKE_REPORTED=garbage
scenario keep-on-failure 3 0 FAKE_REPORTED=3 "INPUT_KEEP-ON-FAILURE=true" KEEP=1
scenario insecure-url 2 0 "INPUT_DAEMON-URL=http://example.com"
scenario bad-egress 2 0 INPUT_EGRESS=sometimes
scenario bad-timeout 2 0 INPUT_TIMEOUT=-1
scenario guest-timeout 124 1 FAKE_REPORTED=124 INPUT_TIMEOUT=30
scenario guest-kill-after-timeout 124 1 FAKE_REPORTED=137 INPUT_TIMEOUT=30

# The limit must reach the guest as a real `timeout` around the job, with the
# client-side backstop strictly longer than it.
grep -q "timeout --kill-after=10 30 sh /tmp/sandkiln-job.sh" "$FAKE_LOG" \
  || { echo "FAIL guest-side timeout missing from the exec-stream wrapper"; failures=$((failures + 1)); }

[ "$failures" -eq 0 ] || { echo "$failures scenario(s) failed"; exit 1; }
echo "all scenarios passed"

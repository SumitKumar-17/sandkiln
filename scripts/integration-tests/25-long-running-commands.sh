# Regression coverage for two bugs found in the 2026-11 audit:
#   1. a command silent for longer than the 3s control-plane vsock bound came
#      back as an error after ~6.5s AND ran twice (the retry re-sent a request
#      the guest had already accepted);
#   2. the daemon-wide sandbox lock was held for the whole call, so one slow
#      command froze every other sandbox operation;
#   3. snapshotting waited ~6s on a `sync` a paused guest can never answer.
section "long-running commands and snapshot latency"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"long-exec"}}')"
assert_status "create sandbox for long-command checks" 200 "$status"
SBX_LONG="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_LONG" ]; then
  fail "create returned no id, aborting long-command checks"
else
  CREATED_SANDBOXES+=("$SBX_LONG")
  marker="/tmp/runs-$$"

  status="$(req POST "/sandboxes/$SBX_LONG/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"echo run >> $marker; sleep 4; echo finished\"]}")"
  assert_status "a command silent for 4s (past the 3s control-plane bound) succeeds" 200 "$status"
  assert_contains "its output arrives" "$(cat "$WORKDIR/resp.json")" "finished"

  req POST "/sandboxes/$SBX_LONG/exec" "{\"command\":\"cat\",\"args\":[\"$marker\"]}" >/dev/null
  assert_eq "the long command ran exactly once, not re-sent after the read timeout" "1" "$(grep -o 'run' "$WORKDIR/resp.json" | wc -l | tr -d ' ')"

  # Another request must not queue behind a running command.
  req POST "/sandboxes/$SBX_LONG/exec" '{"command":"sleep","args":["4"]}' >/dev/null &
  slow_pid=$!
  sleep 1
  started=$(date +%s%N)
  req GET /sandboxes >/dev/null
  elapsed_ms=$(( ($(date +%s%N) - started) / 1000000 ))
  if [ "$elapsed_ms" -lt 1000 ]; then
    pass "GET /sandboxes answered in ${elapsed_ms}ms while another sandbox command was running"
  else
    fail "GET /sandboxes took ${elapsed_ms}ms while a command ran: the sandbox lock is being held across the call"
  fi
  wait "$slow_pid" 2>/dev/null || true

  started=$(date +%s%N)
  status="$(req POST "/sandboxes/$SBX_LONG/snapshot")"
  elapsed_ms=$(( ($(date +%s%N) - started) / 1000000 ))
  assert_status "snapshot the sandbox" 200 "$status"
  SNAP_LONG="$(extract snapshot_id < "$WORKDIR/resp.json")"
  if [ "$elapsed_ms" -lt 3000 ]; then
    pass "snapshot request took ${elapsed_ms}ms (was ~6800ms when it waited on a doomed guest sync)"
  else
    fail "snapshot request took ${elapsed_ms}ms: stopping the paused VM is waiting on a guest sync again"
  fi
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_LONG}")
  [ -n "$SNAP_LONG" ] && { req DELETE "/snapshots/$SNAP_LONG" >/dev/null; }
fi

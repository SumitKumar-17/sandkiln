# Regression coverage for two bugs found in the 2026-11 audit:
#   1. a command silent for longer than the 3s control-plane vsock bound came
#      back as an error after ~6.5s AND ran twice (the retry re-sent a request
#      the guest had already accepted);
#   2. the daemon-wide sandbox lock was held for the whole call, so one slow
#      command froze every other sandbox operation;
#   3. snapshotting waited ~6s on a `sync` a paused guest can never answer.
section "long-running commands and snapshot latency"
status="$(req POST /sandboxes '{"mem_size_mib":128,"tags":{"suite":"integration","case":"long-exec"}}')"
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

  # The bug (stopping a paused VM waited on a guest sync it can never answer)
  # cost 6.5s+ on EVERY snapshot, while load only slows a snapshot some of the
  # time: a snapshot writes the whole guest memory and 10s has been seen under
  # the suite's parallelism. So the check passes if any of up to three
  # snapshots is fast, and fails only if all are slow. An idle box takes ~0.6s.
  # The sandboxes here have 128 MiB of memory on purpose: a snapshot writes the
  # whole guest memory, so a small one stays fast even while the default warm
  # pool is writing its own 512 MiB snapshots in the background, whereas the bug
  # adds 5s+ whatever the size.
  fast_ms=""
  for attempt in 1 2 3; do
    if [ "$attempt" -gt 1 ]; then
      status="$(req POST /sandboxes '{"mem_size_mib":128,"tags":{"suite":"integration","case":"long-exec-retry"}}')"
      SBX_LONG="$(extract id < "$WORKDIR/resp.json")"
      [ -z "$SBX_LONG" ] && break
      CREATED_SANDBOXES+=("$SBX_LONG")
    fi
    started=$(date +%s%N)
    status="$(req POST "/sandboxes/$SBX_LONG/snapshot")"
    elapsed_ms=$(( ($(date +%s%N) - started) / 1000000 ))
    SNAP_LONG="$(extract snapshot_id < "$WORKDIR/resp.json")"
    if [ "$attempt" -eq 1 ]; then assert_status "snapshot the sandbox" 200 "$status"; fi
    if [ "$elapsed_ms" -lt 5500 ]; then fast_ms="$elapsed_ms"; break; fi
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_LONG}")
    [ -n "$SNAP_LONG" ] && { req DELETE "/snapshots/$SNAP_LONG" >/dev/null; SNAP_LONG=""; }
  done
  if [ -n "$fast_ms" ]; then
    pass "snapshot request took ${fast_ms}ms (was ~6800ms when it waited on a doomed guest sync)"
  else
    fail "every snapshot attempt took over 5.5s: stopping the paused VM is waiting on a guest sync again"
  fi
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_LONG}")
  [ -n "$SNAP_LONG" ] && { req DELETE "/snapshots/$SNAP_LONG" >/dev/null; }
fi

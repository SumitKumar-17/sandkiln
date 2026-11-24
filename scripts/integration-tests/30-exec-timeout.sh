# A per-request exec timeout must really kill the command inside the guest,
# not just stop waiting for it: the 504 carries the partial output and
# `timed_out: true`, and nothing (including a shell's grandchildren) is left
# running afterwards. Needs a guest agent that supports `timeout_secs`.
section "exec timeout kills the process in the guest"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"exec-timeout"}}')"
assert_status "create sandbox for exec-timeout checks" 200 "$status"
SBX_TMO="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_TMO" ]; then
  fail "create returned no id, aborting exec-timeout checks"
else
  CREATED_SANDBOXES+=("$SBX_TMO")

  # How many processes named `sleep` the guest has, read from /proc so the
  # check does not depend on procps being in the image.
  guest_sleepers() {
    req POST "/sandboxes/$SBX_TMO/exec" \
      '{"command":"sh","args":["-c","grep -lx sleep /proc/[0-9]*/comm 2>/dev/null | wc -l"]}' >/dev/null
    extract stdout < "$WORKDIR/resp.json" | tr -d '[:space:]'
  }

  # Positive control: the probe above must see a sleep that is running, or its
  # "0" later would prove nothing.
  req POST "/sandboxes/$SBX_TMO/exec" \
    '{"command":"sh","args":["-c","sleep 3 & sleep 0.3; grep -lx sleep /proc/[0-9]*/comm 2>/dev/null | wc -l"]}' >/dev/null
  assert_contains "the sleeper probe sees a running sleep" "$(extract stdout < "$WORKDIR/resp.json" | tr -d "[:space:]")" "1"

  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"sh","args":["-c","echo done; exit 3"],"timeout_secs":5}')"
  assert_status "a command that finishes within its timeout succeeds" 200 "$status"
  assert_contains "its output arrives" "$(cat "$WORKDIR/resp.json")" "done"
  assert_contains "its exit code is the command's own" "$(cat "$WORKDIR/resp.json")" '"exit_code":3'

  started=$(date +%s%N)
  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"sleep","args":["30"],"timeout_secs":2}')"
  elapsed_ms=$(( ($(date +%s%N) - started) / 1000000 ))
  assert_status "sleep 30 with timeout_secs=2 is a 504" 504 "$status"
  assert_contains "the body says it timed out" "$(cat "$WORKDIR/resp.json")" '"timed_out":true'
  assert_contains "the body names the limit" "$(cat "$WORKDIR/resp.json")" '"timeout_secs":2'
  if [ "$elapsed_ms" -ge 1800 ] && [ "$elapsed_ms" -lt 6000 ]; then
    pass "the 504 arrived in ${elapsed_ms}ms, not after the sleep's 30s"
  else
    fail "the timeout took ${elapsed_ms}ms for a 2s limit"
  fi
  assert_eq "the sleep is gone from the guest (it was killed, not abandoned)" "0" "$(guest_sleepers)"

  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"sh","args":["-c","echo partial; echo oops >&2; sleep 30"],"timeout_secs":2}')"
  assert_status "a command that printed before hanging is a 504" 504 "$status"
  assert_contains "the partial stdout is returned" "$(cat "$WORKDIR/resp.json")" '"stdout":"partial\n"'
  assert_contains "the partial stderr is returned" "$(cat "$WORKDIR/resp.json")" '"stderr":"oops\n"'

  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"sh","args":["-c","sleep 40 & sleep 41 & echo started; wait"],"timeout_secs":2}')"
  assert_status "a shell with background children is a 504" 504 "$status"
  assert_contains "the shell's own output is returned" "$(cat "$WORKDIR/resp.json")" "started"
  assert_eq "its grandchildren died with it (the whole process group is killed)" "0" "$(guest_sleepers)"

  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"echo","args":["still alive"],"timeout_secs":5}')"
  assert_status "the sandbox is still usable after a killed command" 200 "$status"
  assert_contains "and answers normally" "$(cat "$WORKDIR/resp.json")" "still alive"

  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"true","timeout_secs":0}')"
  assert_status "timeout_secs=0 is rejected" 400 "$status"
  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"true","timeout_secs":99999999}')"
  assert_status "a timeout above the daemon's exec_timeout_secs is rejected" 400 "$status"
  assert_contains "the error names the limit" "$(cat "$WORKDIR/resp.json")" "exec_timeout_secs"
  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"true","timeout_secs":-1}')"
  assert_status "a negative timeout is rejected" 422 "$status"
  status="$(req POST "/sandboxes/$SBX_TMO/exec" '{"command":"true","timeout_secs":"soon"}')"
  assert_status "a non-numeric timeout is rejected" 422 "$status"
fi

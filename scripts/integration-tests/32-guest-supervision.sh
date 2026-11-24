# What keeps a guest healthy without the daemon's help: the guest agent comes
# back after it is killed, orphaned processes are reaped instead of piling up
# as zombies, and (init-mode images only) signalling PID 1 shuts the VM down
# cleanly and the sandbox can still be deleted. The first two hold for every
# image (systemd restarts the agent, PID 1 reaps); the third needs the agent
# to be PID 1 and is skipped otherwise.
section "guest supervision: agent restart, zombie reaping, PID 1 shutdown"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"guest-supervision"}}')"
assert_status "create sandbox for guest supervision checks" 200 "$status"
SBX_SUP="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_SUP" ]; then
  fail "create returned no id, aborting guest supervision checks"
else
  CREATED_SANDBOXES+=("$SBX_SUP")

  guest_sh() { req POST "/sandboxes/$SBX_SUP/exec" "$(jq -nc --arg c "$1" '{command:"sh",args:["-c",$c]}')"; }
  guest_out() { extract stdout < "$WORKDIR/resp.json" | tr -d '[:space:]'; }
  # Zombies are read from /proc so the probe does not depend on procps.
  zombies() { guest_sh 'cat /proc/[0-9]*/stat 2>/dev/null | grep -c ") Z "' >/dev/null; guest_out; }
  wait_for_zombies() { # <expected> <tries>
    local n
    for _ in $(seq 1 "$2"); do n="$(zombies)"; [ "$n" = "$1" ] && break; sleep 1; done
    echo "$n"
  }

  # --- the agent is restarted after SIGKILL, and the daemon reconnects ---
  guest_sh 'echo $PPID' >/dev/null
  agent_before="$(guest_out)"
  guest_sh 'kill -9 $PPID' >/dev/null   # the exec's parent is the agent itself
  agent_after=""
  for _ in $(seq 1 20); do
    guest_sh 'echo $PPID' >/dev/null
    agent_after="$(guest_out)"
    [ -n "$agent_after" ] && break
    sleep 0.5
  done
  if [ -n "$agent_before" ] && [ -n "$agent_after" ] && [ "$agent_before" != "$agent_after" ]; then
    pass "the agent was killed and came back as a new process ($agent_before -> $agent_after)"
  else
    fail "the agent did not come back as a new process (before '$agent_before', after '$agent_after')"
  fi
  status="$(req POST "/sandboxes/$SBX_SUP/exec" '{"command":"echo","args":["after-agent-kill"]}')"
  assert_status "the sandbox answers normally after the agent restart" 200 "$status"
  assert_contains "and the output is right" "$(cat "$WORKDIR/resp.json")" "after-agent-kill"

  # --- orphans are reaped ---
  # Control: a process whose parent never waits stays a zombie, so the probe
  # can see one. The parent exits after 4s and PID 1 must then reap it.
  guest_sh "nohup sh -c 'sleep 0.1 & exec sleep 4' >/dev/null 2>&1 &" >/dev/null
  sleep 1
  assert_eq "the zombie probe sees a zombie whose parent is not waiting" "1" "$(zombies)"
  assert_eq "PID 1 reaps it once its parent has exited" "0" "$(wait_for_zombies 0 10)"
  guest_sh 'for i in $(seq 200); do sh -c "sleep 0.01 & exit 0" & done; wait' >/dev/null
  assert_eq "no zombies remain after 200 short-lived orphans" "0" "$(wait_for_zombies 0 10)"

  # --- PID 1 shutdown (init-mode images only) ---
  guest_sh 'readlink /proc/1/exe' >/dev/null
  if [[ "$(extract stdout < "$WORKDIR/resp.json")" != *sandkiln-agent* ]]; then
    echo "  skip - PID 1 is not the guest agent on this image, no init-mode shutdown checks"
  else
    pass "PID 1 is the guest agent on this image"
    req POST "/sandboxes/$SBX_SUP/exec" '{"command":"kill","args":["-TERM","1"]}' >/dev/null
    down=""
    for _ in $(seq 1 10); do
      sleep 1
      status="$(req POST "/sandboxes/$SBX_SUP/exec" '{"command":"true"}')"
      [ "$status" != "200" ] && { down=1; break; }
    done
    [ -n "$down" ] && pass "SIGTERM to PID 1 shut the guest down (exec stopped answering)" || fail "the guest still answers 10s after SIGTERM to PID 1"
    status="$(req DELETE "/sandboxes/$SBX_SUP?keep=false")"
    assert_status "a sandbox whose guest shut itself down can still be deleted" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_SUP}")
    status="$(req GET /sandboxes)"
    assert_not_contains "and it is gone from the list" "$(cat "$WORKDIR/resp.json")" "$SBX_SUP"
  fi
fi

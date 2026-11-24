# A guest that shuts itself down (reboot -f, kill -TERM 1, a crash) ends its
# Firecracker process. The daemon must notice and clean the sandbox up: it
# leaves GET /sandboxes, calls to it answer 404 instead of a dead-VM 500, its
# history row ends with `vm_exited`, and its tap and address return to the pools.
section "a guest that shuts itself down is cleaned up"

status="$(req POST /sandboxes '{"mem_size_mib":128,"tags":{"suite":"integration","case":"vm-exit"}}')"
assert_status "create a sandbox" 200 "$status"
SBX_EXIT="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_EXIT" ]; then
  fail "create returned no id, aborting vm-exit checks"
else
  CREATED_SANDBOXES+=("$SBX_EXIT")
  req GET /sandboxes >/dev/null
  assert_contains "the live sandbox is listed" "$(cat "$WORKDIR/resp.json")" "$SBX_EXIT"

  # The guest dies mid-request, so the call itself may fail: only the aftermath matters.
  req POST "/sandboxes/$SBX_EXIT/exec" '{"command":"sh","args":["-c","reboot -f || poweroff -f || echo b > /proc/sysrq-trigger"],"timeout_secs":5}' >/dev/null || true

  gone=""
  for _ in $(seq 1 20); do
    req GET /sandboxes >/dev/null
    if ! grep -q "$SBX_EXIT" "$WORKDIR/resp.json"; then gone="yes"; break; fi
    sleep 1
  done
  if [ -n "$gone" ]; then pass "the dead sandbox left GET /sandboxes without a DELETE"; else fail "the dead sandbox is still listed 20s after its guest shut down"; fi

  status="$(req POST "/sandboxes/$SBX_EXIT/exec" '{"command":"true"}')"
  assert_status "a call to the cleaned-up sandbox is 404, not a dead-VM 500" 404 "$status"

  reason="missing"
  for _ in $(seq 1 10); do
    req GET /sandboxes/history >/dev/null
    reason="$(python3 -c 'import sys,json; sid=sys.argv[1]; recs=json.load(sys.stdin).get("history", []); print(next((r.get("end_reason") for r in recs if r.get("id")==sid), "missing"))' "$SBX_EXIT" < "$WORKDIR/resp.json")"
    [ "$reason" = "vm_exited" ] && break
    sleep 1
  done
  assert_eq "its history row ended as vm_exited" "vm_exited" "$reason"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_EXIT}")
fi

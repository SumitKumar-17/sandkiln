section "resource overrides"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"resource-override"},"vcpu_count":1,"mem_size_mib":256}')"
assert_status "create sandbox with a valid vcpu_count/mem_size_mib override" 200 "$status"
SBX_RES="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_RES" ]; then
  fail "create sandbox with resource override returned no id — aborting override checks"
else
  CREATED_SANDBOXES+=("$SBX_RES")
  pass "create sandbox with resource override returned an id ($SBX_RES)"

  status="$(req POST "/sandboxes/$SBX_RES/exec" '{"command":"nproc","args":[]}')"
  assert_status "exec in resource-overridden sandbox" 200 "$status"
  assert_contains "overridden vcpu_count of 1 is visible inside the guest" "$(cat "$WORKDIR/resp.json")" '"stdout":"1'

  status="$(req DELETE "/sandboxes/$SBX_RES?keep=false")"
  assert_status "stop resource-overridden sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RES}")
fi

status="$(req POST /sandboxes '{"vcpu_count":0}')"
assert_status "vcpu_count of 0 is rejected" 400 "$status"

status="$(req POST /sandboxes '{"mem_size_mib":0}')"
assert_status "mem_size_mib of 0 is rejected" 400 "$status"

status="$(req POST /sandboxes '{"vcpu_count":999}')"
assert_status "vcpu_count above the configured ceiling is rejected" 400 "$status"

status="$(req POST /sandboxes '{"mem_size_mib":999999999}')"
assert_status "mem_size_mib above the configured ceiling is rejected" 400 "$status"

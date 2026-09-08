section "jailer"
if [ -z "${SANDKILN_JAILER_ENABLED:-}" ]; then
  echo "  skip - SANDKILN_JAILER_ENABLED not set in this script's environment, presumed off on the daemon too"
else
  status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"jailer"}}')"
  assert_status "create sandbox under jailer" 200 "$status"
  SBX_JAIL="$(extract id < "$WORKDIR/resp.json")"
  if [ -z "$SBX_JAIL" ]; then
    fail "create sandbox under jailer returned no id — aborting jailer checks"
  else
    CREATED_SANDBOXES+=("$SBX_JAIL")
    pass "create sandbox under jailer returned an id ($SBX_JAIL)"

    status="$(req POST "/sandboxes/$SBX_JAIL/exec" '{"command":"echo","args":["hello-from-a-jailed-vm"]}')"
    assert_status "exec in a jailed sandbox" 200 "$status"
    assert_contains "exec output is correct from inside the chroot" "$(cat "$WORKDIR/resp.json")" "hello-from-a-jailed-vm"

    status="$(req POST "/sandboxes/$SBX_JAIL/snapshot")"
    assert_status "snapshotting a jailed sandbox is rejected" 400 "$status"

    status="$(req DELETE "/sandboxes/$SBX_JAIL")"
    assert_status "stopping a jailed sandbox with the persist-by-default behavior is a conflict (can't be snapshotted)" 409 "$status"
    assert_contains "the conflict message points at the ?keep=false opt-out" "$(cat "$WORKDIR/resp.json")" "keep=false"

    status="$(req DELETE "/sandboxes/$SBX_JAIL?keep=false")"
    assert_status "stop the jailed sandbox with the explicit destroy opt-out" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_JAIL}")
  fi
fi

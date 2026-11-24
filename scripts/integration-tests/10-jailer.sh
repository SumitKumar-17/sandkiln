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

    # A second concurrent jailed sandbox runs under its own uid and chroot:
    # both must work at once, and each only sees its own files.
    status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"jailer-second"}}')"
    assert_status "create a second concurrent jailed sandbox" 200 "$status"
    SBX_JAIL2="$(extract id < "$WORKDIR/resp.json")"
    if [ -n "$SBX_JAIL2" ]; then
      CREATED_SANDBOXES+=("$SBX_JAIL2")
      req POST "/sandboxes/$SBX_JAIL/exec" '{"command":"sh","args":["-c","echo one > /tmp/jail-marker"]}' >/dev/null
      status="$(req POST "/sandboxes/$SBX_JAIL2/exec" '{"command":"sh","args":["-c","cat /tmp/jail-marker 2>&1; echo second-ok"]}')"
      assert_status "exec in the second jailed sandbox" 200 "$status"
      assert_contains "the second jailed sandbox answers" "$(cat "$WORKDIR/resp.json")" "second-ok"
      assert_not_contains "the second sandbox does not see the first one's file" "$(cat "$WORKDIR/resp.json")" "one"
      status="$(req DELETE "/sandboxes/$SBX_JAIL2?keep=false")"
      assert_status "stop the second jailed sandbox" 204 "$status"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_JAIL2}")
    fi

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

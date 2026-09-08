section "snapshot fork: repeatable, non-consuming resume"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"fork"}}')"
assert_status "create sandbox for fork test" 200 "$status"
SBX6="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX6" ]; then
  fail "create sandbox for fork test returned no id — aborting fork checks"
else
  CREATED_SANDBOXES+=("$SBX6")
  FORK_MARKER="fork-marker-$SBX6"

  status="$(req POST "/sandboxes/$SBX6/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"echo $FORK_MARKER > /tmp/fork-marker.txt\"]}")"
  assert_status "write pre-snapshot marker for fork test" 200 "$status"

  status="$(req POST "/sandboxes/$SBX6/snapshot")"
  assert_status "snapshot the sandbox for fork test" 200 "$status"
  FSNAP="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX6}")  # snapshotting consumes the live sandbox

  if [ -z "$FSNAP" ]; then
    fail "fork-test snapshot returned no snapshot_id — aborting fork checks"
  else
    CREATED_SNAPSHOTS+=("$FSNAP")
    pass "fork-test snapshot returned an id ($FSNAP)"

    status="$(req POST "/snapshots/$FSNAP/fork")"
    assert_status "fork the snapshot" 200 "$status"
    FORK1="$(extract id < "$WORKDIR/resp.json")"

    if [ -z "$FORK1" ]; then
      fail "fork returned no sandbox id — aborting remaining fork checks"
    else
      CREATED_SANDBOXES+=("$FORK1")
      pass "fork returned a sandbox id ($FORK1)"

      status="$(req POST "/sandboxes/$FORK1/read-file" '{"path":"/tmp/fork-marker.txt"}')"
      assert_status "read marker file from forked sandbox" 200 "$status"
      decoded="$(extract content_base64 < "$WORKDIR/resp.json" | base64 -d 2>/dev/null || true)"
      assert_contains "marker file content survived fork" "$decoded" "$FORK_MARKER"

      status="$(req GET /snapshots)"
      assert_contains "forked snapshot is still listed (not consumed)" "$(cat "$WORKDIR/resp.json")" "$FSNAP"
      assert_contains "snapshot listing reports the live fork" "$(cat "$WORKDIR/resp.json")" "\"forked_into\":\"$FORK1\""

      status="$(req POST "/snapshots/$FSNAP/fork")"
      assert_status "forking again while a fork is live is a conflict" 409 "$status"

      status="$(req POST "/snapshots/$FSNAP/resume")"
      assert_status "resuming (consuming) while a fork is live is a conflict" 409 "$status"

      status="$(req DELETE "/snapshots/$FSNAP")"
      assert_status "deleting a snapshot while a fork is live is a conflict" 409 "$status"

      status="$(req POST "/sandboxes/$FORK1/snapshot")"
      assert_status "snapshotting a forked sandbox directly is a conflict" 409 "$status"

      status="$(req DELETE "/sandboxes/$FORK1")"
      assert_status "stopping a forked sandbox with the default (persist) behavior still succeeds" 200 "$status"
      assert_contains "a fork has nothing new to preserve, so it's silently destroyed rather than erroring" "$(cat "$WORKDIR/resp.json")" '"kept":false'
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK1}")

      status="$(req POST "/snapshots/$FSNAP/fork")"
      assert_status "fork again after the earlier fork was stopped" 200 "$status"
      FORK2="$(extract id < "$WORKDIR/resp.json")"

      if [ -z "$FORK2" ]; then
        fail "second fork returned no sandbox id"
      else
        CREATED_SANDBOXES+=("$FORK2")
        pass "second fork returned a sandbox id ($FORK2)"

        status="$(req POST "/sandboxes/$FORK2/read-file" '{"path":"/tmp/fork-marker.txt"}')"
        assert_status "read marker file from second fork" 200 "$status"
        decoded="$(extract content_base64 < "$WORKDIR/resp.json" | base64 -d 2>/dev/null || true)"
        assert_contains "second fork starts from the same pristine snapshot state" "$decoded" "$FORK_MARKER"

        status="$(req DELETE "/sandboxes/$FORK2?keep=false")"
        assert_status "stop the second forked sandbox" 204 "$status"
        CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK2}")
      fi

      status="$(req POST "/snapshots/$FSNAP/resume")"
      assert_status "the snapshot can still be consumed via resume once no fork is live" 200 "$status"
      FORK3="$(extract id < "$WORKDIR/resp.json")"
      CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/$FSNAP}")  # resuming consumes the snapshot

      if [ -n "$FORK3" ]; then
        CREATED_SANDBOXES+=("$FORK3")
        status="$(req DELETE "/sandboxes/$FORK3?keep=false")"
        assert_status "stop the finally-resumed sandbox" 204 "$status"
        CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK3}")
      fi
    fi
  fi
fi

status="$(req POST /snapshots/not-a-real-snapshot/fork)"
assert_status "forking a nonexistent snapshot is 404" 404 "$status"

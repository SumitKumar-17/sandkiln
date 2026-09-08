section "snapshot and resume"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"snapshot"}}')"
assert_status "create sandbox for snapshot test" 200 "$status"
SBX4="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX4" ]; then
  fail "create sandbox for snapshot test returned no id — aborting snapshot checks"
else
  CREATED_SANDBOXES+=("$SBX4")
  MARKER="snapshot-marker-$SBX4"

  status="$(req POST "/sandboxes/$SBX4/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"echo $MARKER > /tmp/marker.txt\"]}")"
  assert_status "write pre-snapshot marker" 200 "$status"

  status="$(req POST "/sandboxes/$SBX4/snapshot")"
  assert_status "snapshot the sandbox" 200 "$status"
  SNAP="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX4}")  # snapshotting consumes the live sandbox

  if [ -z "$SNAP" ]; then
    fail "snapshot returned no snapshot_id — aborting resume check"
  else
    CREATED_SNAPSHOTS+=("$SNAP")
    pass "snapshot returned an id ($SNAP)"

    status="$(req GET "/snapshots?source_sandbox_id=$SBX4")"
    assert_status "list snapshots filtered by source_sandbox_id" 200 "$status"
    assert_contains "source_sandbox_id filter finds the snapshot the sandbox became" "$(cat "$WORKDIR/resp.json")" "$SNAP"

    status="$(req GET "/snapshots?source_sandbox_id=not-a-real-sandbox-id")"
    assert_status "list snapshots with a non-matching source_sandbox_id" 200 "$status"
    assert_contains "non-matching source_sandbox_id filter returns an empty list" "$(cat "$WORKDIR/resp.json")" '"snapshots":[]'

    status="$(req POST "/snapshots/$SNAP/resume")"
    assert_status "resume from snapshot" 200 "$status"
    SBX5="$(extract id < "$WORKDIR/resp.json")"
    CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/$SNAP}")  # resuming consumes the snapshot

    if [ -z "$SBX5" ]; then
      fail "resume returned no sandbox id"
    else
      CREATED_SANDBOXES+=("$SBX5")
      pass "resume returned a sandbox id ($SBX5)"

      status="$(req POST "/sandboxes/$SBX5/read-file" '{"path":"/tmp/marker.txt"}')"
      assert_status "read marker file from resumed sandbox" 200 "$status"
      decoded="$(extract content_base64 < "$WORKDIR/resp.json" | base64 -d 2>/dev/null || true)"
      assert_contains "marker file content survived snapshot/resume" "$decoded" "$MARKER"

      status="$(req POST "/sandboxes/$SBX5/exec" '{"command":"echo","args":["post-resume-exec-ok"]}')"
      assert_status "exec still works after resume" 200 "$status"

      status="$(req DELETE "/sandboxes/$SBX5?keep=false")"
      assert_status "stop the resumed sandbox" 204 "$status"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX5}")
    fi
  fi
fi

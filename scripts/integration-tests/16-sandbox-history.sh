section "durable sandbox history (survives a daemon restart)"
# This does NOT restart the daemon mid-suite (that would tear down every
# other sandbox this run still has live) -- it only checks the
# create/destroy/snapshot recording behavior while the daemon is up.
# Actual across-a-real-restart behavior (orphan-marking a still-live
# record, leaving already-ended ones untouched) was verified manually;
# see ROADMAP.md's "Tags and sandbox metadata" section for that result.
status="$(req POST /sandboxes "{\"name\":\"it-hist-live-$$\",\"tags\":{\"suite\":\"integration\",\"case\":\"history\"}}")"
assert_status "create a sandbox to appear in history while still live" 200 "$status"
SBX_HIST_LIVE="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_HIST_LIVE" ]; then
  fail "create sandbox for history test returned no id — aborting history checks"
else
  CREATED_SANDBOXES+=("$SBX_HIST_LIVE")

  status="$(req GET "/sandboxes/history?limit=1000")"
  assert_status "GET /sandboxes/history" 200 "$status"
  hist_body="$(cat "$WORKDIR/resp.json")"
  # Tags serialize from a HashMap -- key order isn't guaranteed, so these
  # are checked as independent substrings rather than one ordered blob.
  assert_contains "history includes the still-live sandbox" "$hist_body" "\"id\":\"$SBX_HIST_LIVE\""
  assert_contains "history entry has the sandbox's real name" "$hist_body" "\"name\":\"it-hist-live-$$\""
  assert_contains "history entry has the sandbox's real tags" "$hist_body" '"suite":"integration"'

  status="$(req GET "/sandboxes/history?live_only=true")"
  assert_status "GET /sandboxes/history?live_only=true" 200 "$status"
  assert_contains "live_only=true includes the still-live sandbox" "$(cat "$WORKDIR/resp.json")" "\"id\":\"$SBX_HIST_LIVE\""

  status="$(req DELETE "/sandboxes/$SBX_HIST_LIVE?keep=false")"
  assert_status "destroy the history-test sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_HIST_LIVE}")

  status="$(req GET "/sandboxes/history?limit=1000")"
  assert_contains "history reflects the destroy as end_reason:destroyed" "$(cat "$WORKDIR/resp.json")" "\"id\":\"$SBX_HIST_LIVE\",\"name\":\"it-hist-live-$$\""
  destroyed_entry="$(cat "$WORKDIR/resp.json")"
  assert_contains "destroyed sandbox's history entry reports the right end_reason" "$destroyed_entry" '"end_reason":"destroyed"'

  status="$(req GET "/sandboxes/history?live_only=true")"
  assert_not_contains "live_only=true no longer includes the now-destroyed sandbox" "$(cat "$WORKDIR/resp.json")" "\"id\":\"$SBX_HIST_LIVE\""
fi

section "durable sandbox history: snapshot recording"
status="$(req POST /sandboxes "{\"name\":\"it-hist-snap-$$\"}")"
assert_status "create a sandbox to snapshot for the history test" 200 "$status"
SBX_HIST_SNAP="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_HIST_SNAP" ]; then
  fail "create sandbox for history-snapshot test returned no id — aborting"
else
  CREATED_SANDBOXES+=("$SBX_HIST_SNAP")

  status="$(req POST "/sandboxes/$SBX_HIST_SNAP/snapshot")"
  assert_status "snapshot the history-test sandbox" 200 "$status"
  HIST_SNAP_ID="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_HIST_SNAP}")

  if [ -n "$HIST_SNAP_ID" ]; then
    CREATED_SNAPSHOTS+=("$HIST_SNAP_ID")
    status="$(req GET "/sandboxes/history?limit=1000")"
    hist_body="$(cat "$WORKDIR/resp.json")"
    assert_contains "history reflects the snapshot as end_reason:snapshotted" "$hist_body" '"end_reason":"snapshotted"'
    assert_contains "history records the resulting snapshot id" "$hist_body" "\"final_snapshot_id\":\"$HIST_SNAP_ID\""
  fi
fi

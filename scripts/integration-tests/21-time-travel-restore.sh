section "time-travel restore (retired snapshot checkpoints)"

# No separate CREATED_RETIRED array: integration-test.sh's own cleanup
# trap sweeps GET /snapshots/history for anything retired during this
# run and deletes it, regardless of which topic file (or which internal,
# untracked path) actually produced it -- see that script's own
# RUN_STARTED_UNIX comment.

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"time-travel-root"}}')"
assert_status "create the root sandbox" 200 "$status"
SBX_ROOT="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_ROOT" ]; then
  fail "create the root sandbox returned no id -- aborting time-travel checks"
else
  CREATED_SANDBOXES+=("$SBX_ROOT")

  status="$(req POST "/sandboxes/$SBX_ROOT/write-file" '{"path":"/marker.txt","content_base64":"b3JpZ2luYWwtY2hlY2twb2ludA=="}')"
  assert_status "write the original marker (base64 for 'original-checkpoint')" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_ROOT/snapshot")"
  assert_status "snapshot the root sandbox" 200 "$status"
  SNAP_ROOT="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_ROOT}")
  [ -n "$SNAP_ROOT" ] && CREATED_SNAPSHOTS+=("$SNAP_ROOT")

  if [ -z "$SNAP_ROOT" ]; then
    fail "snapshotting the root sandbox returned no snapshot_id -- aborting time-travel checks"
  else
    status="$(req POST "/snapshots/$SNAP_ROOT/resume")"
    assert_status "resume the root snapshot (default retain_history=true)" 200 "$status"
    SBX_RESUMED="$(extract id < "$WORKDIR/resp.json")"

    if [ -z "$SBX_RESUMED" ]; then
      fail "resuming the root snapshot returned no id -- aborting time-travel checks"
    else
      CREATED_SANDBOXES+=("$SBX_RESUMED")

      status="$(req GET "/snapshots/history?source_sandbox_id=$SBX_ROOT")"
      assert_status "list retired history filtered by the root sandbox's id" 200 "$status"
      RETIRED_ID="$(jq -r '.checkpoints[0].id // empty' < "$WORKDIR/resp.json")"
      if [ -z "$RETIRED_ID" ]; then
        fail "resuming did not retire a restorable checkpoint by default"
      else
        pass "resuming by default retired a restorable checkpoint ($RETIRED_ID)"

        status="$(req POST "/sandboxes/$SBX_RESUMED/write-file" '{"path":"/marker.txt","content_base64":"bXV0YXRlZC1hZnRlci1yZXN1bWU="}')"
        assert_status "mutate the marker in the resumed (current) sandbox" 204 "$status"

        status="$(req POST "/snapshots/history/$RETIRED_ID/restore")"
        assert_status "restoring while the current descendant is still live is refused" 409 "$status"

        status="$(req DELETE "/sandboxes/$SBX_RESUMED?keep=false")"
        assert_status "destroy the current (mutated) sandbox, freeing the checkpoint's network identity" 204 "$status"
        CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RESUMED}")

        status="$(req POST "/snapshots/history/$RETIRED_ID/restore")"
        assert_status "restore the checkpoint now that nothing else holds its network identity" 200 "$status"
        SBX_RESTORED="$(extract id < "$WORKDIR/resp.json")"

        if [ -z "$SBX_RESTORED" ]; then
          fail "restoring the checkpoint returned no id -- aborting the rest of this check"
        else
          CREATED_SANDBOXES+=("$SBX_RESTORED")

          status="$(req POST "/sandboxes/$SBX_RESTORED/read-file" '{"path":"/marker.txt"}')"
          assert_status "read the marker back from the restored sandbox" 200 "$status"
          content="$(jq -r '.content_base64 // empty' < "$WORKDIR/resp.json" | base64 -d 2>/dev/null)"
          assert_eq "the restored sandbox has the ORIGINAL content, not the later mutation" "original-checkpoint" "$content"

          status="$(req POST "/sandboxes/$SBX_RESTORED/snapshot")"
          assert_status "a restored sandbox stays snapshottable (owns its lease/rootfs outright, unlike a fork)" 200 "$status"
          SNAP_FROM_RESTORE="$(extract snapshot_id < "$WORKDIR/resp.json")"
          CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RESTORED}")

          if [ -n "$SNAP_FROM_RESTORE" ]; then
            status="$(req GET "/snapshots?source_sandbox_id=$SBX_RESTORED")"
            parent="$(jq -r '.snapshots[0].parent_snapshot_id // empty' < "$WORKDIR/resp.json")"
            assert_eq "the new snapshot's lineage correctly points back at the restored checkpoint" "$RETIRED_ID" "$parent"

            # This new snapshot still holds the checkpoint's shared
            # network identity (a `Snapshot` holds its lease the whole
            # time it exists, same as ever) -- deleted here, not tracked
            # in CREATED_SNAPSHOTS, specifically so the next check below
            # (restoring the *original* checkpoint again) isn't refused
            # by a conflict with this descendant of its own first restore.
            status="$(req DELETE "/snapshots/$SNAP_FROM_RESTORE")"
            assert_status "delete the snapshot descended from the first restore, freeing the shared network identity" 204 "$status"
          fi
        fi

        # Restoring is non-consuming: the same checkpoint must still be
        # restorable again once nothing else holds its network identity.
        status="$(req POST "/snapshots/history/$RETIRED_ID/restore")"
        assert_status "the same checkpoint can be restored a second time (not consumed by the first restore)" 200 "$status"
        SBX_RESTORED_AGAIN="$(extract id < "$WORKDIR/resp.json")"
        if [ -n "$SBX_RESTORED_AGAIN" ]; then
          CREATED_SANDBOXES+=("$SBX_RESTORED_AGAIN")
          status="$(req DELETE "/sandboxes/$SBX_RESTORED_AGAIN?keep=false")"
          assert_status "destroy the second restore" 204 "$status"
          CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RESTORED_AGAIN}")
        fi
      fi
    fi
  fi
fi

status="$(req POST /snapshots/history/nonexistent-checkpoint-id/restore)"
assert_status "restoring a nonexistent checkpoint is 404" 404 "$status"

status="$(req DELETE /snapshots/history/nonexistent-checkpoint-id)"
assert_status "deleting a nonexistent checkpoint is 404" 404 "$status"

section "time-travel restore: deleting a retired checkpoint reclaims its disk usage"

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"time-travel-delete"}}')"
SBX_FOR_DELETE="$(extract id < "$WORKDIR/resp.json")"
if [ -n "$SBX_FOR_DELETE" ]; then
  CREATED_SANDBOXES+=("$SBX_FOR_DELETE")
  status="$(req POST "/sandboxes/$SBX_FOR_DELETE/snapshot")"
  SNAP_FOR_DELETE="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_FOR_DELETE}")

  if [ -n "$SNAP_FOR_DELETE" ]; then
    status="$(req POST "/snapshots/$SNAP_FOR_DELETE/resume")"
    SBX_FOR_DELETE_RESUMED="$(extract id < "$WORKDIR/resp.json")"
    if [ -n "$SBX_FOR_DELETE_RESUMED" ]; then
      CREATED_SANDBOXES+=("$SBX_FOR_DELETE_RESUMED")
      status="$(req DELETE "/sandboxes/$SBX_FOR_DELETE_RESUMED?keep=false")"
      assert_status "destroy the current descendant, freeing the checkpoint's network identity" 204 "$status"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_FOR_DELETE_RESUMED}")

      status="$(req DELETE "/snapshots/history/$SNAP_FOR_DELETE")"
      assert_status "delete the retired checkpoint" 204 "$status"

      status="$(req GET "/snapshots/history?source_sandbox_id=$SBX_FOR_DELETE")"
      assert_not_contains "the deleted checkpoint no longer appears in history" "$(cat "$WORKDIR/resp.json")" "\"id\":\"$SNAP_FOR_DELETE\""

      status="$(req POST "/snapshots/history/$SNAP_FOR_DELETE/restore")"
      assert_status "restoring a deleted checkpoint is 404" 404 "$status"
    fi
  fi
fi

section "time-travel restore: ?retain_history=false opts out"

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"time-travel-opt-out"}}')"
SBX_OPT_OUT="$(extract id < "$WORKDIR/resp.json")"
if [ -n "$SBX_OPT_OUT" ]; then
  CREATED_SANDBOXES+=("$SBX_OPT_OUT")
  status="$(req POST "/sandboxes/$SBX_OPT_OUT/snapshot")"
  SNAP_OPT_OUT="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_OPT_OUT}")
  # Tracked even though retain_history=false consumes it below (leaving
  # nothing at either DELETE target) -- harmless, and correct in case a
  # future edit to this test ever short-circuits before that point.
  [ -n "$SNAP_OPT_OUT" ] && CREATED_SNAPSHOTS+=("$SNAP_OPT_OUT")

  if [ -n "$SNAP_OPT_OUT" ]; then
    status="$(req POST "/snapshots/$SNAP_OPT_OUT/resume?retain_history=false")"
    assert_status "resume with retain_history=false" 200 "$status"
    SBX_OPT_OUT_RESUMED="$(extract id < "$WORKDIR/resp.json")"
    [ -n "$SBX_OPT_OUT_RESUMED" ] && CREATED_SANDBOXES+=("$SBX_OPT_OUT_RESUMED")

    status="$(req GET "/snapshots/history?source_sandbox_id=$SBX_OPT_OUT")"
    assert_status "list retired history for the opted-out resume" 200 "$status"
    assert_not_contains "retain_history=false left nothing restorable behind" "$(cat "$WORKDIR/resp.json")" "\"source_sandbox_id\":\"$SBX_OPT_OUT\""

    status="$(req POST "/snapshots/$SNAP_OPT_OUT/resume")"
    # retain_history=false consumed it exactly like the pre-existing
    # default behavior always has -- a second resume attempt is 404.
    assert_status "the opted-out snapshot is gone after being consumed, same as before this feature existed" 404 "$status"
  fi
fi

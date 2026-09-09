section "snapshot lineage (parent_snapshot_id)"

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"lineage-root"}}')"
assert_status "create the root sandbox" 200 "$status"
SBX_ROOT="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_ROOT" ]; then
  fail "create the root sandbox returned no id -- aborting lineage checks"
else
  CREATED_SANDBOXES+=("$SBX_ROOT")

  status="$(req POST "/sandboxes/$SBX_ROOT/snapshot")"
  assert_status "snapshot the root sandbox" 200 "$status"
  SNAP_ROOT="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_ROOT}")

  if [ -z "$SNAP_ROOT" ]; then
    fail "snapshotting the root sandbox returned no snapshot_id -- aborting lineage checks"
  else
    CREATED_SNAPSHOTS+=("$SNAP_ROOT")

    status="$(req GET "/snapshots?source_sandbox_id=$SBX_ROOT")"
    assert_status "list the root snapshot by its source sandbox" 200 "$status"
    root_parent="$(jq -r '.snapshots[0].parent_snapshot_id // "null"' < "$WORKDIR/resp.json")"
    assert_eq "a cold-booted sandbox's snapshot has no parent (it's a lineage root)" "null" "$root_parent"

    status="$(req POST "/snapshots/$SNAP_ROOT/resume")"
    assert_status "resume the root snapshot" 200 "$status"
    SBX_RESUMED="$(extract id < "$WORKDIR/resp.json")"
    CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/$SNAP_ROOT}")

    if [ -z "$SBX_RESUMED" ]; then
      fail "resuming the root snapshot returned no id -- aborting the resume-lineage check"
    else
      CREATED_SANDBOXES+=("$SBX_RESUMED")

      status="$(req POST "/sandboxes/$SBX_RESUMED/snapshot")"
      assert_status "snapshot the resumed sandbox (a resumed sandbox stays snapshottable)" 200 "$status"
      SNAP_CHILD="$(extract snapshot_id < "$WORKDIR/resp.json")"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RESUMED}")

      if [ -z "$SNAP_CHILD" ]; then
        fail "snapshotting the resumed sandbox returned no snapshot_id -- aborting the resume-lineage check"
      else
        CREATED_SNAPSHOTS+=("$SNAP_CHILD")

        status="$(req GET "/snapshots?parent_snapshot_id=$SNAP_ROOT")"
        assert_status "query children of the (now-consumed) root snapshot" 200 "$status"
        assert_contains "the resume-produced snapshot is found by its parent's id" "$(cat "$WORKDIR/resp.json")" "$SNAP_CHILD"
      fi
    fi
  fi
fi

# A forked (not resumed) sandbox cannot itself be re-snapshotted (see
# routes_snapshot.rs's module doc comment on why -- it shares its
# snapshot's live rootfs file) so the fork side of lineage tracking
# (Sandbox::parent_snapshot_id set at fork time) has no observable
# snapshot-to-snapshot effect to check here; this only re-confirms that
# existing, unrelated restriction still holds.
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"lineage-fork-root"}}')"
SBX_FORK_ROOT="$(extract id < "$WORKDIR/resp.json")"
if [ -n "$SBX_FORK_ROOT" ]; then
  CREATED_SANDBOXES+=("$SBX_FORK_ROOT")
  status="$(req POST "/sandboxes/$SBX_FORK_ROOT/snapshot")"
  SNAP_FORK_ROOT="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_FORK_ROOT}")
  if [ -n "$SNAP_FORK_ROOT" ]; then
    CREATED_SNAPSHOTS+=("$SNAP_FORK_ROOT")
    status="$(req POST "/snapshots/$SNAP_FORK_ROOT/fork")"
    assert_status "fork the snapshot" 200 "$status"
    SBX_FORKED="$(extract id < "$WORKDIR/resp.json")"
    if [ -n "$SBX_FORKED" ]; then
      CREATED_SANDBOXES+=("$SBX_FORKED")
      status="$(req POST "/sandboxes/$SBX_FORKED/snapshot")"
      assert_status "a forked sandbox is still refused for snapshotting, as before this feature" 409 "$status"
    fi
  fi
fi

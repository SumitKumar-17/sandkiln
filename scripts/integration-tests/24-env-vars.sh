section "per-sandbox environment variables"

# Create-time env is the base layer; a per-call env wins on a key
# conflict and can also add keys the create-time env never had -- see
# routes_exec::resolve_env's own doc comment for the merge semantics
# this exercises.

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"env-vars"},"env":{"BASE_VAR":"base-value","SHARED":"from-create"}}')"
assert_status "create sandbox with env" 200 "$status"
SBX_ENV="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_ENV" ]; then
  fail "create sandbox with env returned no id -- aborting env-var checks"
else
  CREATED_SANDBOXES+=("$SBX_ENV")

  status="$(req POST "/sandboxes/$SBX_ENV/exec" '{"command":"sh","args":["-c","echo BASE_VAR=$BASE_VAR SHARED=$SHARED CALL_VAR=$CALL_VAR"]}')"
  assert_status "exec with no per-call env still succeeds" 200 "$status"
  assert_contains "create-time env reaches the exec'd process unchanged" "$(cat "$WORKDIR/resp.json")" "BASE_VAR=base-value SHARED=from-create CALL_VAR="

  status="$(req POST "/sandboxes/$SBX_ENV/exec" '{"command":"sh","args":["-c","echo BASE_VAR=$BASE_VAR SHARED=$SHARED CALL_VAR=$CALL_VAR"],"env":{"SHARED":"from-call","CALL_VAR":"call-value"}}')"
  assert_status "exec with a per-call env override succeeds" 200 "$status"
  exec_env_body="$(cat "$WORKDIR/resp.json")"
  assert_contains "per-call env wins on a key conflict (SHARED)" "$exec_env_body" "SHARED=from-call"
  assert_contains "per-call env adds a new key (CALL_VAR) without erasing the base layer (BASE_VAR)" "$exec_env_body" "BASE_VAR=base-value"
  assert_contains "per-call env's new key reaches the process" "$exec_env_body" "CALL_VAR=call-value"

  status="$(req POST "/sandboxes/$SBX_ENV/snapshot")"
  assert_status "snapshot a sandbox with env" 200 "$status"
  SNAP_ENV="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_ENV}")

  if [ -z "$SNAP_ENV" ]; then
    fail "snapshotting an env-bearing sandbox returned no snapshot_id -- aborting resume/fork checks"
  else
    CREATED_SNAPSHOTS+=("$SNAP_ENV")

    status="$(req POST "/snapshots/$SNAP_ENV/fork")"
    assert_status "fork a snapshot with env" 200 "$status"
    FORK_ENV="$(extract id < "$WORKDIR/resp.json")"
    [ -n "$FORK_ENV" ] && CREATED_SANDBOXES+=("$FORK_ENV")
    if [ -n "$FORK_ENV" ]; then
      status="$(req POST "/sandboxes/$FORK_ENV/exec" '{"command":"sh","args":["-c","echo BASE_VAR=$BASE_VAR SHARED=$SHARED"]}')"
      assert_status "exec in the forked sandbox succeeds" 200 "$status"
      assert_contains "env survives fork unchanged" "$(cat "$WORKDIR/resp.json")" "BASE_VAR=base-value SHARED=from-create"
      # The source snapshot refuses to resume while a live fork holds its
      # lease/rootfs (see Snapshot::forked_into's own doc comment) --
      # stop the fork first so the resume check below isn't testing that
      # lock instead of what it means to.
      status="$(req DELETE "/sandboxes/$FORK_ENV?keep=false")"
      assert_status "stop the forked sandbox, freeing the snapshot to resume" 204 "$status"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK_ENV}")
    else
      fail "fork returned no id -- skipping the forked sandbox's env check"
    fi

    status="$(req POST "/snapshots/$SNAP_ENV/resume")"
    assert_status "resume the snapshot with env" 200 "$status"
    RESUME_ENV="$(extract id < "$WORKDIR/resp.json")"
    CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/$SNAP_ENV}")
    if [ -n "$RESUME_ENV" ]; then
      CREATED_SANDBOXES+=("$RESUME_ENV")
      status="$(req POST "/sandboxes/$RESUME_ENV/exec" '{"command":"sh","args":["-c","echo BASE_VAR=$BASE_VAR SHARED=$SHARED"]}')"
      assert_status "exec in the resumed sandbox succeeds" 200 "$status"
      assert_contains "env survives resume unchanged" "$(cat "$WORKDIR/resp.json")" "BASE_VAR=base-value SHARED=from-create"
    else
      fail "resume returned no id -- skipping the resumed sandbox's env check"
    fi
  fi
fi

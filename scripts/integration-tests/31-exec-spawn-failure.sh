# A command that cannot be started is a normal result with the shell's exit
# code (127 not found, 126 not executable) and the reason on stderr, not an
# HTTP 500, on the plain, the timed and the streamed exec path. A mount on a
# sandbox without rclone or /dev/fuse says which one is missing. Needs a guest
# agent that maps spawn failures.
section "exec of a command that cannot start"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"exec-spawn-failure"}}')"
assert_status "create sandbox for spawn-failure checks" 200 "$status"
SBX_SPAWN="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_SPAWN" ]; then
  fail "create returned no id, aborting spawn-failure checks"
else
  CREATED_SANDBOXES+=("$SBX_SPAWN")

  for variant in plain timed; do
    limit=""
    [ "$variant" = timed ] && limit=',"timeout_secs":5'

    status="$(req POST "/sandboxes/$SBX_SPAWN/exec" "{\"command\":\"sk-no-such-command\"$limit}")"
    assert_status "$variant: a missing command is a 200, not a 500" 200 "$status"
    assert_contains "$variant: exit code 127" "$(cat "$WORKDIR/resp.json")" '"exit_code":127'
    assert_contains "$variant: stderr says command not found" "$(cat "$WORKDIR/resp.json")" 'sk-no-such-command: command not found'

    status="$(req POST "/sandboxes/$SBX_SPAWN/exec" "{\"command\":\"/no/such/path\"$limit}")"
    assert_status "$variant: a missing path is a 200" 200 "$status"
    assert_contains "$variant: missing path is 127" "$(cat "$WORKDIR/resp.json")" '"exit_code":127'
    assert_contains "$variant: stderr names the path" "$(cat "$WORKDIR/resp.json")" '/no/such/path: No such file or directory'

    status="$(req POST "/sandboxes/$SBX_SPAWN/exec" "{\"command\":\"/tmp\"$limit}")"
    assert_status "$variant: a directory is a 200" 200 "$status"
    assert_contains "$variant: a directory is 126" "$(cat "$WORKDIR/resp.json")" '"exit_code":126'
    assert_contains "$variant: stderr says it is a directory" "$(cat "$WORKDIR/resp.json")" '/tmp: Is a directory'

    req POST "/sandboxes/$SBX_SPAWN/exec" '{"command":"sh","args":["-c","echo data > /tmp/sk-plain.txt; chmod 644 /tmp/sk-plain.txt"]}' >/dev/null
    status="$(req POST "/sandboxes/$SBX_SPAWN/exec" "{\"command\":\"/tmp/sk-plain.txt\"$limit}")"
    assert_status "$variant: a non-executable file is a 200" 200 "$status"
    assert_contains "$variant: a non-executable file is 126" "$(cat "$WORKDIR/resp.json")" '"exit_code":126'
    assert_contains "$variant: stderr says permission denied" "$(cat "$WORKDIR/resp.json")" 'Permission denied'
  done

  status="$(req POST "/sandboxes/$SBX_SPAWN/exec" '{"command":"sh","args":["-c","exit 127"]}')"
  assert_status "a command that itself exits 127 is unchanged" 200 "$status"
  assert_contains "and carries no spawn message" "$(cat "$WORKDIR/resp.json")" '"stderr":""'

  status="$(req POST "/sandboxes/$SBX_SPAWN/exec" '{"command":"echo","args":["still fine"]}')"
  assert_status "the sandbox is still usable afterwards" 200 "$status"
  assert_contains "and answers normally" "$(cat "$WORKDIR/resp.json")" "still fine"

  # Make the mount preconditions fail deterministically on this throwaway
  # sandbox whatever the image ships: move rclone aside (remembering where it
  # was), then remove the FUSE device node.
  req POST "/sandboxes/$SBX_SPAWN/exec" '{"command":"sh","args":["-c","p=$(command -v rclone) && { echo \"$p\" > /tmp/sk-rclone-path; mv \"$p\" \"$p.off\"; }; true"]}' >/dev/null
  MOUNT_BODY='{"bucket":"b","endpoint":"http://127.0.0.1:1","access_key":"k","secret_key":"s","mount_path":"/mnt/sk-spawn"}'
  status="$(req POST "/sandboxes/$SBX_SPAWN/mounts" "$MOUNT_BODY")"
  assert_status "mounting without rclone is a 400" 400 "$status"
  assert_contains "the error names the missing rclone binary" "$(cat "$WORKDIR/resp.json")" "rclone binary"

  req POST "/sandboxes/$SBX_SPAWN/exec" '{"command":"sh","args":["-c","[ -f /tmp/sk-rclone-path ] && p=$(cat /tmp/sk-rclone-path) && mv \"$p.off\" \"$p\"; rm -f /dev/fuse; true"]}' >/dev/null
  req POST "/sandboxes/$SBX_SPAWN/exec" '{"command":"sh","args":["-c","[ -f /tmp/sk-rclone-path ] && echo yes || echo no"]}' >/dev/null
  had_rclone="$(extract stdout < "$WORKDIR/resp.json" | tr -d '[:space:]')"
  status="$(req POST "/sandboxes/$SBX_SPAWN/mounts" "$MOUNT_BODY")"
  assert_status "mounting without /dev/fuse (or rclone) is a 400" 400 "$status"
  if [ "$had_rclone" = yes ]; then
    assert_contains "the error names the missing /dev/fuse" "$(cat "$WORKDIR/resp.json")" "/dev/fuse"
  else
    echo "  skip - this image has no rclone to put back, so the /dev/fuse message is not reachable"
  fi
  status="$(req GET "/sandboxes/$SBX_SPAWN/mounts")"
  assert_eq "a refused mount leaves no mount behind" '{"mounts":[]}' "$(tr -d '[:space:]' < "$WORKDIR/resp.json")"
fi

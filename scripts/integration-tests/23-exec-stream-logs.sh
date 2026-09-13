section "streamed background exec sessions (exec-stream / logs)"
# Same reasoning as 17-pty.sh: no clean way to drive a WebSocket from
# bash+curl, so this delegates to a small Node helper
# (scripts/lib/exec-stream-check.mjs, Node's own native WebSocket).
# Skipped, not failed, if node isn't on PATH.
if ! command -v node >/dev/null 2>&1; then
  echo "  skip - node not found on PATH, skipping exec-stream/logs WebSocket checks"
else
  status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"exec-stream"}}')"
  assert_status "create sandbox for exec-stream test" 200 "$status"
  SBX_STREAM="$(extract id < "$WORKDIR/resp.json")"
  if [ -z "$SBX_STREAM" ]; then
    fail "create sandbox for exec-stream test returned no id -- aborting exec-stream checks"
  else
    CREATED_SANDBOXES+=("$SBX_STREAM")

    if node "$SCRIPT_DIR/lib/exec-stream-check.mjs" "$BASE_URL" "$SBX_STREAM" "$AUTH_TOKEN" >"$WORKDIR/exec-stream-output.txt" 2>&1; then
      pass "exec-stream: replay-then-live-tail while running, and reattach-after-finish, both verified"
    else
      fail "exec-stream WebSocket check ($(tr '\n' ' ' <"$WORKDIR/exec-stream-output.txt"))"
    fi

    status="$(req GET "/sandboxes/$SBX_STREAM/exec-stream")"
    assert_status "list exec-stream sessions" 200 "$status"
    assert_contains "the finished session is listed with a 0 exit code" "$(cat "$WORKDIR/resp.json")" '"exit_code":0'

    status="$(req POST "/sandboxes/$SBX_STREAM/exec-stream" '{"command":""}')"
    assert_status "an empty command is rejected" 400 "$status"

    status="$(req DELETE "/sandboxes/$SBX_STREAM?keep=false")"
    assert_status "stop the exec-stream-test sandbox" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_STREAM}")
  fi
fi

section "interactive terminal (PTY over WebSocket)"
# The bash+curl harness the rest of this suite uses has no clean way to
# drive a WebSocket, so this delegates to a small Node helper
# (scripts/lib/pty-check.mjs, Node's own native WebSocket) instead of
# inventing a bash-native WebSocket client. Skipped, not failed, if node
# isn't on PATH -- every other topic here only needs curl/jq.
if ! command -v node >/dev/null 2>&1; then
  echo "  skip - node not found on PATH, skipping PTY WebSocket checks"
else
  status="$(req POST /sandboxes "{\"tags\":{\"suite\":\"integration\",\"case\":\"pty\"}}")"
  assert_status "create sandbox for PTY test" 200 "$status"
  SBX_PTY="$(extract id < "$WORKDIR/resp.json")"
  if [ -z "$SBX_PTY" ]; then
    fail "create sandbox for PTY test returned no id — aborting PTY checks"
  else
    CREATED_SANDBOXES+=("$SBX_PTY")

    if node "$SCRIPT_DIR/lib/pty-check.mjs" "$BASE_URL" "$SBX_PTY" "$AUTH_TOKEN" >"$WORKDIR/pty-output.txt" 2>&1; then
      pass "PTY WebSocket session: a real command's output round-tripped through a real shell"
    else
      fail "PTY WebSocket session round-trip ($(tr '\n' ' ' <"$WORKDIR/pty-output.txt"))"
    fi

    # Open a second session and hang up without telling the shell to
    # exit -- exercises sandkiln-guest-agent's pty.rs SIGHUP-on-hangup
    # cleanup (see its own AGENTS.md). "pts" filters out the baseline
    # ttyS0 login shell, which is always present and would otherwise
    # make this assertion pass even if the real fix regressed.
    if node "$SCRIPT_DIR/lib/pty-check.mjs" --disconnect-only "$BASE_URL" "$SBX_PTY" "$AUTH_TOKEN" >"$WORKDIR/pty-disconnect.txt" 2>&1; then
      pass "PTY WebSocket disconnected cleanly without waiting for the shell to exit"
    else
      fail "PTY disconnect-only session ($(tr '\n' ' ' <"$WORKDIR/pty-disconnect.txt"))"
    fi
    sleep 1
    status="$(req POST "/sandboxes/$SBX_PTY/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"ps aux | grep '[b]ash' | grep -c pts\"]}")"
    assert_status "exec ps check after PTY disconnect" 200 "$status"
    orphan_count="$(extract stdout <"$WORKDIR/resp.json" | tr -d '[:space:]')"
    assert_eq "no orphaned shell left running after a PTY client disconnects first" "0" "$orphan_count"

    status="$(req DELETE "/sandboxes/$SBX_PTY?keep=false")"
    assert_status "stop the PTY-test sandbox" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_PTY}")
  fi
fi

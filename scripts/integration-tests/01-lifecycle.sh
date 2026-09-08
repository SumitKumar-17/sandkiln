section "sandbox lifecycle"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"lifecycle"}}')"
assert_status "create sandbox" 200 "$status"
SBX1="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX1" ]; then
  fail "create sandbox returned no id — aborting lifecycle checks"
else
  CREATED_SANDBOXES+=("$SBX1")
  pass "create sandbox returned an id ($SBX1)"

  status="$(req GET "/sandboxes?tag.case=lifecycle")"
  assert_status "list sandboxes filtered by tag" 200 "$status"
  assert_contains "tag-filtered list includes the new sandbox" "$(cat "$WORKDIR/resp.json")" "$SBX1"

  status="$(req GET "/sandboxes?tag.case=not-a-real-tag-value")"
  assert_contains "tag filter excludes non-matching sandboxes" "$(cat "$WORKDIR/resp.json")" '"sandboxes":[]'

  status="$(req POST "/sandboxes/$SBX1/exec" '{"command":"echo","args":["hello-integration-test"]}')"
  assert_status "exec in sandbox" 200 "$status"
  assert_contains "exec stdout is correct" "$(cat "$WORKDIR/resp.json")" "hello-integration-test"

  status="$(req POST "/sandboxes/$SBX1/write-file" '{"path":"/tmp/it.txt","content_base64":"aXQtd29ya3M="}')"
  assert_status "write-file" 204 "$status"

  status="$(req POST "/sandboxes/$SBX1/read-file" '{"path":"/tmp/it.txt"}')"
  assert_status "read-file" 200 "$status"
  assert_contains "read-file returns what was written" "$(cat "$WORKDIR/resp.json")" "aXQtd29ya3M="

  status="$(req POST "/sandboxes/$SBX1/exec" '{"command":"false","args":[]}')"
  body="$(cat "$WORKDIR/resp.json")"
  assert_status "exec of a failing command still returns 200" 200 "$status"
  assert_contains "exec reports the real non-zero exit code" "$body" '"exit_code":1'

  status="$(req DELETE "/sandboxes/$SBX1?keep=false")"
  assert_status "stop sandbox (explicit ?keep=false, unrelated to persistence — see below)" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX1}")

  status="$(req GET /sandboxes)"
  assert_not_contains "stopped sandbox no longer listed" "$(cat "$WORKDIR/resp.json")" "$SBX1"
fi

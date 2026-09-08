section "named sandboxes and persistent stop by default"
NAME1="it-name-$$-1"

status="$(req POST /sandboxes "{\"tags\":{\"suite\":\"integration\",\"case\":\"name\"},\"name\":\"$NAME1\"}")"
assert_status "create sandbox with a name" 200 "$status"
SBXN1="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBXN1" ]; then
  fail "create sandbox with a name returned no id — aborting naming checks"
else
  CREATED_SANDBOXES+=("$SBXN1")
  pass "create sandbox with a name returned an id ($SBXN1)"

  status="$(req POST /sandboxes "{\"name\":\"$NAME1\"}")"
  assert_status "creating a second sandbox with an already-taken name is a conflict" 409 "$status"

  status="$(req GET "/sandboxes/by-name/$NAME1")"
  assert_status "resolve a live sandbox by name" 200 "$status"
  assert_eq "by-name resolves to the sandbox's real id" "$SBXN1" "$(extract id < "$WORKDIR/resp.json")"

  NAME_MARKER="name-marker-$SBXN1"
  status="$(req POST "/sandboxes/$SBXN1/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"echo $NAME_MARKER > /tmp/name-marker.txt\"]}")"
  assert_status "write pre-stop marker in the named sandbox" 200 "$status"

  status="$(req DELETE "/sandboxes/$SBXN1")"
  assert_status "stopping a named sandbox with the default behavior preserves it (200, not 204)" 200 "$status"
  body="$(cat "$WORKDIR/resp.json")"
  assert_contains "the default stop reports kept:true" "$body" '"kept":true'
  NAME_SNAP="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBXN1}")

  if [ -z "$NAME_SNAP" ]; then
    fail "default stop returned no snapshot_id — aborting resume-by-name checks"
  else
    CREATED_SNAPSHOTS+=("$NAME_SNAP")
    pass "default stop returned a snapshot id ($NAME_SNAP)"

    status="$(req GET "/sandboxes/by-name/$NAME1")"
    assert_status "by-name no longer resolves live once stopped (state preserved, not destroyed)" 409 "$status"

    status="$(req POST /sandboxes/get-or-create "{\"name\":\"$NAME1\"}")"
    assert_status "get-or-create resumes a stopped sandbox found by name" 200 "$status"
    body="$(cat "$WORKDIR/resp.json")"
    assert_contains "get-or-create reports created:false when resuming" "$body" '"created":false'
    SBXN1B="$(extract id < "$WORKDIR/resp.json")"
    CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/$NAME_SNAP}")  # resuming consumes the snapshot

    if [ -z "$SBXN1B" ]; then
      fail "get-or-create resume returned no sandbox id — aborting remaining naming checks"
    else
      CREATED_SANDBOXES+=("$SBXN1B")
      pass "get-or-create resume returned a sandbox id ($SBXN1B)"

      status="$(req POST "/sandboxes/$SBXN1B/read-file" '{"path":"/tmp/name-marker.txt"}')"
      assert_status "read marker file from the name-resumed sandbox" 200 "$status"
      decoded="$(extract content_base64 < "$WORKDIR/resp.json" | base64 -d 2>/dev/null || true)"
      assert_contains "marker file content survived a stop-then-resume-by-name cycle" "$decoded" "$NAME_MARKER"

      status="$(req POST /sandboxes/get-or-create "{\"name\":\"$NAME1\"}")"
      assert_status "get-or-create on a now-live name succeeds" 200 "$status"
      body="$(cat "$WORKDIR/resp.json")"
      assert_contains "get-or-create on an already-live name reports created:false" "$body" '"created":false'
      assert_eq "get-or-create is idempotent: same id for an already-live name" "$SBXN1B" "$(extract id < "$WORKDIR/resp.json")"

      status="$(req DELETE "/sandboxes/$SBXN1B?keep=false")"
      assert_status "the explicit ?keep=false opt-out actually destroys" 204 "$status"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBXN1B}")

      status="$(req GET "/sandboxes/by-name/$NAME1")"
      assert_status "the name resolves to nothing at all after an explicit destroy" 404 "$status"
    fi
  fi
fi

NAME2="it-name-$$-2"
status="$(req POST /sandboxes/get-or-create "{\"name\":\"$NAME2\"}")"
assert_status "get-or-create on a brand-new name creates fresh" 200 "$status"
body="$(cat "$WORKDIR/resp.json")"
assert_contains "get-or-create on a brand-new name reports created:true" "$body" '"created":true'
SBXN2="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBXN2" ]; then
  fail "get-or-create on a brand-new name returned no id — aborting idempotency check"
else
  CREATED_SANDBOXES+=("$SBXN2")
  pass "get-or-create on a brand-new name returned an id ($SBXN2)"

  status="$(req POST /sandboxes/get-or-create "{\"name\":\"$NAME2\"}")"
  assert_status "get-or-create is idempotent for an already-live name (second call)" 200 "$status"
  body="$(cat "$WORKDIR/resp.json")"
  assert_contains "the repeat call reports created:false" "$body" '"created":false'
  assert_eq "the repeat call returns the same sandbox id" "$SBXN2" "$(extract id < "$WORKDIR/resp.json")"

  status="$(req DELETE "/sandboxes/$SBXN2?keep=false")"
  assert_status "clean up the get-or-create sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBXN2}")
fi

status="$(req GET "/sandboxes/by-name/not-a-real-name-ever-used")"
assert_status "by-name for a name that was never used is 404" 404 "$status"

status="$(req POST /sandboxes '{"name":""}')"
assert_status "an empty name is rejected" 400 "$status"

status="$(req POST /sandboxes/get-or-create '{}')"
assert_status "get-or-create with no name is rejected" 400 "$status"

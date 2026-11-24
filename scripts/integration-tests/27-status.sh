# GET /status: the operator health document. Compares exact structure, never
# global counts (concurrent topics create and stop sandboxes while this runs).
section "daemon status"
status="$(req GET /status)"
assert_status "GET /status answers" 200 "$status"
body="$(cat "$WORKDIR/resp.json")"
for key in version uptime_secs counts tap_pool rootfs_clones disk warnings; do
  assert_contains "status has '$key'" "$body" "\"$key\""
done

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"status"}}')"
assert_status "create sandbox for the status check" 200 "$status"
SBX_STATUS="$(extract id < "$WORKDIR/resp.json")"
if [ -n "$SBX_STATUS" ]; then
  CREATED_SANDBOXES+=("$SBX_STATUS")
  req GET /status >/dev/null
  live="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["counts"]["sandboxes"])' < "$WORKDIR/resp.json")"
  if [ "$live" -ge 1 ]; then pass "status counts the live sandbox ($live)"; else fail "status reports $live live sandboxes with one running"; fi
  req GET /status >/dev/null
  free="$(python3 -c 'import json,sys; d=json.load(sys.stdin)["tap_pool"]; print(d["size"]-d["free"])' < "$WORKDIR/resp.json")"
  if [ "$free" -ge 1 ]; then pass "status shows a tap device in use"; else fail "no tap device shown in use while a sandbox runs"; fi
else
  fail "create returned no id, aborting status checks"
fi

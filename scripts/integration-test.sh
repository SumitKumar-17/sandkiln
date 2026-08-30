#!/usr/bin/env bash
# End-to-end integration test against a running sandkilnd daemon: exercises
# the full API surface in one repeatable run — sandbox lifecycle, tags,
# drives (including persistence across sandboxes and conflict detection),
# snapshot/resume/fork (including the one-live-fork-at-a-time conflict
# rules), auth, metrics, and error cases — instead of re-deriving
# the same manual curl session by hand every time a feature is touched.
#
# This does not replace `cargo test` (pure logic, no KVM needed) or
# `scripts/load-test.sh` (concurrency/latency under load) — it's the third
# leg: does the whole real system, wired together, actually behave
# correctly end to end. Every case here was, at some point, a bug found by
# doing this exact sequence by hand (see AGENTS.md's gotchas list and
# ROADMAP.md's Benchmarking section for the history) — codifying it means
# the next feature doesn't get to reopen one of these by accident.
#
# Usage: scripts/integration-test.sh [base-url]
#   base-url defaults to $SANDKILN_INTEGRATION_TEST_URL or
#   http://127.0.0.1:7777. If SANDKILN_AUTH_TOKEN is set in the environment,
#   auth-specific checks run too; otherwise they're skipped with a note,
#   since a daemon started without SANDKILN_AUTH_TOKEN can't be told to
#   require one at request time. Likewise, if SANDKILN_JAILER_ENABLED is
#   set in this script's own environment, jailer-specific checks run too
#   (exec inside a jailed sandbox, and that snapshotting one is rejected) —
#   set it to whatever value the daemon under test was actually started
#   with jailer enabled via, so this only runs those checks when they can
#   actually pass.
#
# Every sandbox/drive/snapshot this script creates is tracked and torn
# down on exit, pass or fail — see the cleanup trap near the bottom.

set -uo pipefail

BASE_URL="${1:-${SANDKILN_INTEGRATION_TEST_URL:-http://127.0.0.1:7777}}"
BASE_URL="${BASE_URL%/}"
AUTH_TOKEN="${SANDKILN_AUTH_TOKEN:-}"

WORKDIR="$(mktemp -d /tmp/sandkiln-integration-test-XXXXXX)"
PASS=0
FAIL=0
FAILURES=()

CREATED_SANDBOXES=()
CREATED_DRIVES=()
CREATED_SNAPSHOTS=()

cleanup() {
  for id in "${CREATED_SANDBOXES[@]:-}"; do
    [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/sandboxes/$id" "${AUTH_HEADER[@]}"
  done
  for id in "${CREATED_SNAPSHOTS[@]:-}"; do
    [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/snapshots/$id" "${AUTH_HEADER[@]}"
  done
  for id in "${CREATED_DRIVES[@]:-}"; do
    [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/drives/$id" "${AUTH_HEADER[@]}"
  done
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

AUTH_HEADER=()
[ -n "$AUTH_TOKEN" ] && AUTH_HEADER=(-H "Authorization: Bearer $AUTH_TOKEN")

extract() {
  # extract <json-field> — reads stdin, prints that top-level string field.
  local field="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r ".${field} // empty"
  else
    grep -o "\"${field}\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | sed -E 's/.*:"([^"]*)"/\1/'
  fi
}

req() {
  # req <method> <path> [json-body] — writes the response body to
  # $WORKDIR/resp.json, prints the HTTP status code.
  local method="$1" path="$2" data="${3:-}"
  if [ -n "$data" ]; then
    curl -s -o "$WORKDIR/resp.json" -w '%{http_code}' -X "$method" "$BASE_URL$path" \
      -H 'Content-Type: application/json' "${AUTH_HEADER[@]}" -d "$data"
  else
    curl -s -o "$WORKDIR/resp.json" -w '%{http_code}' -X "$method" "$BASE_URL$path" "${AUTH_HEADER[@]}"
  fi
}

pass() {
  PASS=$((PASS + 1))
  echo "  ok   - $1"
}

fail() {
  FAIL=$((FAIL + 1))
  FAILURES+=("$1")
  echo "  FAIL - $1"
}

assert_status() {
  # assert_status <description> <expected> <actual>
  if [ "$3" = "$2" ]; then
    pass "$1"
  else
    fail "$1 (expected http $2, got $3; body: $(cat "$WORKDIR/resp.json" 2>/dev/null | head -c 200))"
  fi
}

assert_eq() {
  # assert_eq <description> <expected> <actual>
  if [ "$2" = "$3" ]; then
    pass "$1"
  else
    fail "$1 (expected '$2', got '$3')"
  fi
}

assert_contains() {
  # assert_contains <description> <haystack> <needle>
  if [[ "$2" == *"$3"* ]]; then
    pass "$1"
  else
    fail "$1 (expected to find '$3')"
  fi
}

assert_not_contains() {
  # assert_not_contains <description> <haystack> <needle>
  if [[ "$2" != *"$3"* ]]; then
    pass "$1"
  else
    fail "$1 (expected NOT to find '$3')"
  fi
}

section() { echo; echo "=== $1 ==="; }

# ---------------------------------------------------------------------------
if ! curl -sf -o /dev/null "$BASE_URL/healthz"; then
  echo "sandkilnd not reachable at $BASE_URL/healthz — is it running?" >&2
  exit 1
fi
echo "sandkiln integration test against $BASE_URL"
[ -z "$AUTH_TOKEN" ] && echo "(SANDKILN_AUTH_TOKEN not set — auth checks will be skipped)"

section "health and metrics"
status="$(req GET /healthz)"
assert_status "GET /healthz" 200 "$status"

status="$(req GET /metrics)"
assert_status "GET /metrics (unauthenticated, even with auth on)" 200 "$status"
metrics_body="$(cat "$WORKDIR/resp.json")"
assert_contains "metrics include sandboxes_created_total" "$metrics_body" "sandboxes_created_total"
assert_contains "metrics include sandboxes_active" "$metrics_body" "sandboxes_active"

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

  status="$(req DELETE "/sandboxes/$SBX1")"
  assert_status "stop sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX1}")

  status="$(req GET /sandboxes)"
  assert_not_contains "stopped sandbox no longer listed" "$(cat "$WORKDIR/resp.json")" "$SBX1"
fi

section "error cases"
status="$(req POST "/sandboxes/not-a-real-id/exec" '{"command":"echo","args":[]}')"
assert_status "exec against a nonexistent sandbox is 404" 404 "$status"

status="$(req DELETE "/sandboxes/not-a-real-id")"
assert_status "stop a nonexistent sandbox is 404" 404 "$status"

status="$(req DELETE "/drives/not-a-real-drive")"
assert_status "delete a nonexistent drive is 404" 404 "$status"

section "drives: create, persist across sandboxes, conflict detection"
status="$(req POST /drives '{"size_mib":64}')"
assert_status "create drive" 200 "$status"
DRV="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$DRV" ]; then
  fail "create drive returned no id — aborting drive checks"
else
  CREATED_DRIVES+=("$DRV")
  pass "create drive returned an id ($DRV)"

  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV\"}]}")"
  assert_status "create sandbox with the drive attached" 200 "$status"
  SBX2="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX2")

  status="$(req POST "/sandboxes/$SBX2/exec" '{"command":"sh","args":["-c","mkdir -p /mnt && mkfs.ext4 -F /dev/vdb && mount /dev/vdb /mnt && echo persisted-via-drive > /mnt/marker.txt && umount /mnt"]}')"
  assert_status "format, mount, and write to the attached drive" 200 "$status"
  assert_contains "format/mount/write chain exited 0" "$(cat "$WORKDIR/resp.json")" '"exit_code":0'

  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV\"}]}")"
  assert_status "attaching an already-attached drive to a second sandbox is a conflict" 409 "$status"

  status="$(req DELETE "/sandboxes/$SBX2")"
  assert_status "stop the sandbox holding the drive" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX2}")

  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV\"}]}")"
  assert_status "re-attach the same drive to a new sandbox after release" 200 "$status"
  SBX3="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX3")

  status="$(req POST "/sandboxes/$SBX3/exec" '{"command":"sh","args":["-c","mkdir -p /mnt && mount /dev/vdb /mnt && cat /mnt/marker.txt"]}')"
  assert_status "mount the reattached drive" 200 "$status"
  assert_contains "data written earlier is still there" "$(cat "$WORKDIR/resp.json")" "persisted-via-drive"

  status="$(req DELETE "/sandboxes/$SBX3")"
  assert_status "stop the sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX3}")

  status="$(req DELETE "/drives/$DRV")"
  assert_status "delete the drive now that nothing holds it" 204 "$status"
  CREATED_DRIVES=("${CREATED_DRIVES[@]/$DRV}")
fi

section "snapshot and resume"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"snapshot"}}')"
assert_status "create sandbox for snapshot test" 200 "$status"
SBX4="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX4" ]; then
  fail "create sandbox for snapshot test returned no id — aborting snapshot checks"
else
  CREATED_SANDBOXES+=("$SBX4")
  MARKER="snapshot-marker-$SBX4"

  status="$(req POST "/sandboxes/$SBX4/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"echo $MARKER > /tmp/marker.txt\"]}")"
  assert_status "write pre-snapshot marker" 200 "$status"

  status="$(req POST "/sandboxes/$SBX4/snapshot")"
  assert_status "snapshot the sandbox" 200 "$status"
  SNAP="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX4}")  # snapshotting consumes the live sandbox

  if [ -z "$SNAP" ]; then
    fail "snapshot returned no snapshot_id — aborting resume check"
  else
    CREATED_SNAPSHOTS+=("$SNAP")
    pass "snapshot returned an id ($SNAP)"

    status="$(req POST "/snapshots/$SNAP/resume")"
    assert_status "resume from snapshot" 200 "$status"
    SBX5="$(extract id < "$WORKDIR/resp.json")"
    CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/$SNAP}")  # resuming consumes the snapshot

    if [ -z "$SBX5" ]; then
      fail "resume returned no sandbox id"
    else
      CREATED_SANDBOXES+=("$SBX5")
      pass "resume returned a sandbox id ($SBX5)"

      status="$(req POST "/sandboxes/$SBX5/read-file" '{"path":"/tmp/marker.txt"}')"
      assert_status "read marker file from resumed sandbox" 200 "$status"
      decoded="$(extract content_base64 < "$WORKDIR/resp.json" | base64 -d 2>/dev/null || true)"
      assert_contains "marker file content survived snapshot/resume" "$decoded" "$MARKER"

      status="$(req POST "/sandboxes/$SBX5/exec" '{"command":"echo","args":["post-resume-exec-ok"]}')"
      assert_status "exec still works after resume" 200 "$status"

      status="$(req DELETE "/sandboxes/$SBX5")"
      assert_status "stop the resumed sandbox" 204 "$status"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX5}")
    fi
  fi
fi

section "dev server preview"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"preview"}}')"
assert_status "create sandbox for preview test" 200 "$status"
SBXP="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBXP" ]; then
  fail "create sandbox for preview test returned no id — aborting preview checks"
else
  CREATED_SANDBOXES+=("$SBXP")

  # A real dev server, backgrounded and detached from the exec call's own
  # stdout/stderr pipes (redirected to a log file instead) so `sh -c`
  # exits immediately once the server has forked, rather than blocking on
  # a pipe the backgrounded process would otherwise still be holding open.
  status="$(req POST "/sandboxes/$SBXP/exec" '{"command":"sh","args":["-c","cd /tmp && python3 -m http.server 8123 </dev/null >/tmp/preview-server.log 2>&1 & sleep 1"]}')"
  assert_status "start a dev server inside the sandbox" 200 "$status"

  status="$(req GET "/sandboxes/$SBXP/preview/8123/")"
  assert_status "preview proxies a real HTTP response from the guest" 200 "$status"
  assert_contains "preview response body is the guest server's own content" "$(cat "$WORKDIR/resp.json")" "Directory listing for /"

  status="$(req GET "/sandboxes/not-a-real-id/preview/8123/")"
  assert_status "preview against a nonexistent sandbox is 404" 404 "$status"

  status="$(req GET "/sandboxes/$SBXP/preview/59999/")"
  assert_status "preview against a port nothing is listening on is 502" 502 "$status"

  if [ -n "$AUTH_TOKEN" ]; then
    status="$(curl -s -o "$WORKDIR/resp.json" -w '%{http_code}' "$BASE_URL/sandboxes/$SBXP/preview/8123/")"
    assert_status "preview with no token at all is rejected" 401 "$status"

    status="$(curl -s -o "$WORKDIR/resp.json" -w '%{http_code}' "$BASE_URL/sandboxes/$SBXP/preview/8123/?token=$AUTH_TOKEN")"
    assert_status "preview accepts the token as a query parameter (no Authorization header)" 200 "$status"
  else
    echo "  skip - SANDKILN_AUTH_TOKEN not set, preview auth checks skipped (see the 'auth' section below)"
  fi

  status="$(req DELETE "/sandboxes/$SBXP")"
  assert_status "stop the preview sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBXP}")
fi

section "jailer"
if [ -z "${SANDKILN_JAILER_ENABLED:-}" ]; then
  echo "  skip - SANDKILN_JAILER_ENABLED not set in this script's environment, presumed off on the daemon too"
else
  status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"jailer"}}')"
  assert_status "create sandbox under jailer" 200 "$status"
  SBX_JAIL="$(extract id < "$WORKDIR/resp.json")"
  if [ -z "$SBX_JAIL" ]; then
    fail "create sandbox under jailer returned no id — aborting jailer checks"
  else
    CREATED_SANDBOXES+=("$SBX_JAIL")
    pass "create sandbox under jailer returned an id ($SBX_JAIL)"

    status="$(req POST "/sandboxes/$SBX_JAIL/exec" '{"command":"echo","args":["hello-from-a-jailed-vm"]}')"
    assert_status "exec in a jailed sandbox" 200 "$status"
    assert_contains "exec output is correct from inside the chroot" "$(cat "$WORKDIR/resp.json")" "hello-from-a-jailed-vm"

    status="$(req POST "/sandboxes/$SBX_JAIL/snapshot")"
    assert_status "snapshotting a jailed sandbox is rejected" 400 "$status"

    status="$(req DELETE "/sandboxes/$SBX_JAIL")"
    assert_status "stop the jailed sandbox" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_JAIL}")
  fi
fi

section "snapshot fork: repeatable, non-consuming resume"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"fork"}}')"
assert_status "create sandbox for fork test" 200 "$status"
SBX6="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX6" ]; then
  fail "create sandbox for fork test returned no id — aborting fork checks"
else
  CREATED_SANDBOXES+=("$SBX6")
  FORK_MARKER="fork-marker-$SBX6"

  status="$(req POST "/sandboxes/$SBX6/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"echo $FORK_MARKER > /tmp/fork-marker.txt\"]}")"
  assert_status "write pre-snapshot marker for fork test" 200 "$status"

  status="$(req POST "/sandboxes/$SBX6/snapshot")"
  assert_status "snapshot the sandbox for fork test" 200 "$status"
  FSNAP="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX6}")  # snapshotting consumes the live sandbox

  if [ -z "$FSNAP" ]; then
    fail "fork-test snapshot returned no snapshot_id — aborting fork checks"
  else
    CREATED_SNAPSHOTS+=("$FSNAP")
    pass "fork-test snapshot returned an id ($FSNAP)"

    status="$(req POST "/snapshots/$FSNAP/fork")"
    assert_status "fork the snapshot" 200 "$status"
    FORK1="$(extract id < "$WORKDIR/resp.json")"

    if [ -z "$FORK1" ]; then
      fail "fork returned no sandbox id — aborting remaining fork checks"
    else
      CREATED_SANDBOXES+=("$FORK1")
      pass "fork returned a sandbox id ($FORK1)"

      status="$(req POST "/sandboxes/$FORK1/read-file" '{"path":"/tmp/fork-marker.txt"}')"
      assert_status "read marker file from forked sandbox" 200 "$status"
      decoded="$(extract content_base64 < "$WORKDIR/resp.json" | base64 -d 2>/dev/null || true)"
      assert_contains "marker file content survived fork" "$decoded" "$FORK_MARKER"

      status="$(req GET /snapshots)"
      assert_contains "forked snapshot is still listed (not consumed)" "$(cat "$WORKDIR/resp.json")" "$FSNAP"
      assert_contains "snapshot listing reports the live fork" "$(cat "$WORKDIR/resp.json")" "\"forked_into\":\"$FORK1\""

      status="$(req POST "/snapshots/$FSNAP/fork")"
      assert_status "forking again while a fork is live is a conflict" 409 "$status"

      status="$(req POST "/snapshots/$FSNAP/resume")"
      assert_status "resuming (consuming) while a fork is live is a conflict" 409 "$status"

      status="$(req DELETE "/snapshots/$FSNAP")"
      assert_status "deleting a snapshot while a fork is live is a conflict" 409 "$status"

      status="$(req POST "/sandboxes/$FORK1/snapshot")"
      assert_status "snapshotting a forked sandbox directly is a conflict" 409 "$status"

      status="$(req DELETE "/sandboxes/$FORK1")"
      assert_status "stop the forked sandbox" 204 "$status"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK1}")

      status="$(req POST "/snapshots/$FSNAP/fork")"
      assert_status "fork again after the earlier fork was stopped" 200 "$status"
      FORK2="$(extract id < "$WORKDIR/resp.json")"

      if [ -z "$FORK2" ]; then
        fail "second fork returned no sandbox id"
      else
        CREATED_SANDBOXES+=("$FORK2")
        pass "second fork returned a sandbox id ($FORK2)"

        status="$(req POST "/sandboxes/$FORK2/read-file" '{"path":"/tmp/fork-marker.txt"}')"
        assert_status "read marker file from second fork" 200 "$status"
        decoded="$(extract content_base64 < "$WORKDIR/resp.json" | base64 -d 2>/dev/null || true)"
        assert_contains "second fork starts from the same pristine snapshot state" "$decoded" "$FORK_MARKER"

        status="$(req DELETE "/sandboxes/$FORK2")"
        assert_status "stop the second forked sandbox" 204 "$status"
        CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK2}")
      fi

      status="$(req POST "/snapshots/$FSNAP/resume")"
      assert_status "the snapshot can still be consumed via resume once no fork is live" 200 "$status"
      FORK3="$(extract id < "$WORKDIR/resp.json")"
      CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/$FSNAP}")  # resuming consumes the snapshot

      if [ -n "$FORK3" ]; then
        CREATED_SANDBOXES+=("$FORK3")
        status="$(req DELETE "/sandboxes/$FORK3")"
        assert_status "stop the finally-resumed sandbox" 204 "$status"
        CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK3}")
      fi
    fi
  fi
fi

status="$(req POST /snapshots/not-a-real-snapshot/fork)"
assert_status "forking a nonexistent snapshot is 404" 404 "$status"

section "auth"
if [ -z "$AUTH_TOKEN" ]; then
  echo "  skip - SANDKILN_AUTH_TOKEN not set, daemon presumably running without auth"
else
  saved_header=("${AUTH_HEADER[@]}")
  AUTH_HEADER=()
  status="$(req POST /sandboxes '{}')"
  assert_status "create without a token is rejected" 401 "$status"
  AUTH_HEADER=(-H "Authorization: Bearer wrong-token-entirely")
  status="$(req POST /sandboxes '{}')"
  assert_status "create with the wrong token is rejected" 401 "$status"
  AUTH_HEADER=("${saved_header[@]}")
  status="$(req POST /sandboxes '{}')"
  assert_status "create with the correct token succeeds" 200 "$status"
  SBX_AUTH="$(extract id < "$WORKDIR/resp.json")"
  [ -n "$SBX_AUTH" ] && CREATED_SANDBOXES+=("$SBX_AUTH")
fi

section "results"
echo "passed: $PASS   failed: $FAIL"
if [ "$FAIL" -gt 0 ]; then
  echo
  echo "failures:"
  for f in "${FAILURES[@]}"; do
    echo "  - $f"
  done
fi

[ "$FAIL" -eq 0 ]

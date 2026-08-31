#!/usr/bin/env bash
# End-to-end integration test against a running sandkilnd daemon: exercises
# the full API surface in one repeatable run — sandbox lifecycle, tags,
# per-sandbox resource overrides and their ceiling, drives (including
# persistence across sandboxes and conflict detection), snapshot/resume/
# fork (including the one-live-fork-at-a-time conflict rules), named
# sandboxes and persistent-by-default stop (name-conflict rejection,
# stop-preserves-by-default verified via read-file after a stop+resume-
# by-name cycle, the explicit ?keep=false destroy opt-out, and
# get-or-create-by-name idempotency), registered images (POST/GET
# /images, booting a sandbox via image_id, and in-use deletion refusal),
# request id correlation, auth, metrics, and error cases — instead of
# re-deriving the same manual curl session by hand every time a feature
# is touched.
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
CREATED_IMAGES=()

cleanup() {
  # ?keep=false: cleanup means "get rid of everything this run created",
  # not "leave a snapshot behind" — DELETE now preserves state by
  # default (see the "named sandboxes and persistent stop" section
  # below), which would otherwise leak an untracked snapshot on every
  # run of this script.
  for id in "${CREATED_SANDBOXES[@]:-}"; do
    [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/sandboxes/$id?keep=false" "${AUTH_HEADER[@]}"
  done
  for id in "${CREATED_SNAPSHOTS[@]:-}"; do
    [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/snapshots/$id" "${AUTH_HEADER[@]}"
  done
  for id in "${CREATED_DRIVES[@]:-}"; do
    [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/drives/$id" "${AUTH_HEADER[@]}"
  done
  for id in "${CREATED_IMAGES[@]:-}"; do
    [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/images/$id" "${AUTH_HEADER[@]}"
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

  status="$(req DELETE "/sandboxes/$SBX1?keep=false")"
  assert_status "stop sandbox (explicit ?keep=false, unrelated to persistence — see below)" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX1}")

  status="$(req GET /sandboxes)"
  assert_not_contains "stopped sandbox no longer listed" "$(cat "$WORKDIR/resp.json")" "$SBX1"
fi

section "resource overrides"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"resource-override"},"vcpu_count":1,"mem_size_mib":256}')"
assert_status "create sandbox with a valid vcpu_count/mem_size_mib override" 200 "$status"
SBX_RES="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_RES" ]; then
  fail "create sandbox with resource override returned no id — aborting override checks"
else
  CREATED_SANDBOXES+=("$SBX_RES")
  pass "create sandbox with resource override returned an id ($SBX_RES)"

  status="$(req POST "/sandboxes/$SBX_RES/exec" '{"command":"nproc","args":[]}')"
  assert_status "exec in resource-overridden sandbox" 200 "$status"
  assert_contains "overridden vcpu_count of 1 is visible inside the guest" "$(cat "$WORKDIR/resp.json")" '"stdout":"1'

  status="$(req DELETE "/sandboxes/$SBX_RES?keep=false")"
  assert_status "stop resource-overridden sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RES}")
fi

status="$(req POST /sandboxes '{"vcpu_count":0}')"
assert_status "vcpu_count of 0 is rejected" 400 "$status"

status="$(req POST /sandboxes '{"mem_size_mib":0}')"
assert_status "mem_size_mib of 0 is rejected" 400 "$status"

status="$(req POST /sandboxes '{"vcpu_count":999}')"
assert_status "vcpu_count above the configured ceiling is rejected" 400 "$status"

status="$(req POST /sandboxes '{"mem_size_mib":999999999}')"
assert_status "mem_size_mib above the configured ceiling is rejected" 400 "$status"

section "request id correlation"
resp_headers="$(curl -s -D - -o /dev/null "$BASE_URL/healthz")"
assert_contains "a request with no X-Request-Id gets one generated and echoed back" "$resp_headers" "x-request-id:"

custom_request_id="integration-test-$(date +%s)-$$"
resp_headers="$(curl -s -D - -o /dev/null -H "X-Request-Id: $custom_request_id" "$BASE_URL/healthz")"
assert_contains "a caller-supplied X-Request-Id is echoed back verbatim" "$resp_headers" "$custom_request_id"

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

  status="$(req DELETE "/sandboxes/$SBX2?keep=false")"
  assert_status "stop the sandbox holding the drive" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX2}")

  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV\"}]}")"
  assert_status "re-attach the same drive to a new sandbox after release" 200 "$status"
  SBX3="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX3")

  status="$(req POST "/sandboxes/$SBX3/exec" '{"command":"sh","args":["-c","mkdir -p /mnt && mount /dev/vdb /mnt && cat /mnt/marker.txt"]}')"
  assert_status "mount the reattached drive" 200 "$status"
  assert_contains "data written earlier is still there" "$(cat "$WORKDIR/resp.json")" "persisted-via-drive"

  status="$(req DELETE "/sandboxes/$SBX3?keep=false")"
  assert_status "stop the sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX3}")

  status="$(req DELETE "/drives/$DRV")"
  assert_status "delete the drive now that nothing holds it" 204 "$status"
  CREATED_DRIVES=("${CREATED_DRIVES[@]/$DRV}")
fi

section "drives: concurrent read-only attachment"
status="$(req POST /drives '{"size_mib":64}')"
assert_status "create drive for read-only sharing test" 200 "$status"
DRV_RO="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$DRV_RO" ]; then
  fail "create drive for read-only sharing test returned no id — aborting read-only sharing checks"
else
  CREATED_DRIVES+=("$DRV_RO")
  pass "create drive for read-only sharing test returned an id ($DRV_RO)"

  # Seed real data onto the drive read-write before anything ever attaches
  # it read-only — a drive Firecracker marks read-only for the guest can't
  # be formatted or written to, so the data both read-only holders below
  # verify has to already be there.
  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV_RO\"}]}")"
  assert_status "create sandbox to seed the shared drive" 200 "$status"
  SBX_SEED="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX_SEED")

  status="$(req POST "/sandboxes/$SBX_SEED/exec" '{"command":"sh","args":["-c","mkdir -p /mnt && mkfs.ext4 -F /dev/vdb && mount /dev/vdb /mnt && echo shared-read-only-data > /mnt/marker.txt && umount /mnt"]}')"
  assert_status "format, mount, and seed the drive before read-only sharing" 200 "$status"

  status="$(req DELETE "/sandboxes/$SBX_SEED")"
  assert_status "stop the seeding sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_SEED}")

  # The actual bug this section exists to catch: two sandboxes attaching
  # the same drive read-only at the same time must both succeed — neither
  # can corrupt a drive neither can write to.
  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV_RO\",\"read_only\":true}]}")"
  assert_status "first concurrent read-only attach succeeds" 200 "$status"
  SBX_RO1="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX_RO1")

  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV_RO\",\"read_only\":true}]}")"
  assert_status "second concurrent read-only attach also succeeds while the first is still live" 200 "$status"
  SBX_RO2="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX_RO2")

  status="$(req POST "/sandboxes/$SBX_RO1/exec" '{"command":"sh","args":["-c","mkdir -p /mnt && mount -o ro /dev/vdb /mnt && cat /mnt/marker.txt"]}')"
  assert_status "first read-only holder can mount and read" 200 "$status"
  assert_contains "first read-only holder sees the data seeded before either attached" "$(cat "$WORKDIR/resp.json")" "shared-read-only-data"

  status="$(req POST "/sandboxes/$SBX_RO2/exec" '{"command":"sh","args":["-c","mkdir -p /mnt && mount -o ro /dev/vdb /mnt && cat /mnt/marker.txt"]}')"
  assert_status "second read-only holder can mount and read" 200 "$status"
  assert_contains "second read-only holder sees the same data" "$(cat "$WORKDIR/resp.json")" "shared-read-only-data"

  # A read-write attach must still be refused while even one read-only
  # holder is alive — read-only sharing is not the same as no exclusivity.
  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV_RO\"}]}")"
  assert_status "a read-write attach is rejected while two read-only holders are live" 409 "$status"

  status="$(req DELETE "/sandboxes/$SBX_RO1")"
  assert_status "stop the first read-only holder" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RO1}")

  # Failure-path check: stopping one read-only holder must free exactly
  # its own hold, no more and no less — the drive stays refused for a
  # read-write attach because the second read-only holder is still live,
  # but still accepts a third concurrent read-only attach.
  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV_RO\"}]}")"
  assert_status "a read-write attach is still rejected with one read-only holder remaining" 409 "$status"

  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV_RO\",\"read_only\":true}]}")"
  assert_status "a third read-only attach is still accepted with one read-only holder remaining" 200 "$status"
  SBX_RO3="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX_RO3")

  status="$(req DELETE "/sandboxes/$SBX_RO2")"
  assert_status "stop the second read-only holder" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RO2}")

  status="$(req DELETE "/sandboxes/$SBX_RO3")"
  assert_status "stop the third read-only holder" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RO3}")

  # Once every read-only holder is stopped, the drive must correctly
  # become available again immediately — no stale holder left behind by
  # a force-stop's teardown path.
  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV_RO\",\"read_only\":true}]}")"
  assert_status "a fresh read-only attach succeeds once every prior holder is stopped" 200 "$status"
  SBX_RO4="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX_RO4")

  status="$(req DELETE "/sandboxes/$SBX_RO4")"
  assert_status "stop the final read-only holder" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RO4}")

  status="$(req DELETE "/drives/$DRV_RO")"
  assert_status "delete the read-only-shared drive now that nothing holds it" 204 "$status"
  CREATED_DRIVES=("${CREATED_DRIVES[@]/$DRV_RO}")
fi

section "images: register, boot a sandbox from it, in-use deletion refusal"
# Reuses the daemon's own currently-configured base rootfs (copied to a
# second path) as the "custom" image under test — building or fetching a
# genuinely distinct second image isn't practical for a scripted run, and
# the daemon can't tell the difference: image_id just changes which file
# create_sandbox clones from. Read from this script's own environment,
# same convention as the SANDKILN_JAILER_ENABLED/SANDKILN_AUTH_TOKEN
# checks elsewhere in this file — expanded the same way
# core/crates/daemon/src/config.rs::expand_home does.
BASE_ROOTFS_FOR_IMAGE_TEST="${SANDKILN_BASE_ROOTFS:-~/sandkiln-tools/images/ubuntu-22.04.ext4}"
BASE_ROOTFS_FOR_IMAGE_TEST="${BASE_ROOTFS_FOR_IMAGE_TEST/#\~/$HOME}"
if [ ! -f "$BASE_ROOTFS_FOR_IMAGE_TEST" ]; then
  echo "  skip - SANDKILN_BASE_ROOTFS ($BASE_ROOTFS_FOR_IMAGE_TEST) not reachable from this script's own environment, skipping image checks"
else
  IMAGE_SOURCE_COPY="$WORKDIR/image-source-copy.ext4"
  cp --reflink=auto "$BASE_ROOTFS_FOR_IMAGE_TEST" "$IMAGE_SOURCE_COPY" 2>/dev/null ||
    cp "$BASE_ROOTFS_FOR_IMAGE_TEST" "$IMAGE_SOURCE_COPY"
  IMG_ID="integration-test-image-$$"

  status="$(req POST /images "{\"id\":\"$IMG_ID\",\"path\":\"$IMAGE_SOURCE_COPY\"}")"
  assert_status "register an image from an existing rootfs" 200 "$status"
  registered_id="$(extract id < "$WORKDIR/resp.json")"
  if [ "$registered_id" != "$IMG_ID" ]; then
    fail "register image returned unexpected id ('$registered_id', wanted '$IMG_ID') — aborting image checks"
  else
    CREATED_IMAGES+=("$IMG_ID")
    pass "register image returned the requested id ($IMG_ID)"
    assert_contains "registration response is explicit the guest agent isn't verified" \
      "$(cat "$WORKDIR/resp.json")" '"guest_agent_verified":false'

    status="$(req GET /images)"
    assert_status "list images" 200 "$status"
    assert_contains "listed images include the new one" "$(cat "$WORKDIR/resp.json")" "$IMG_ID"

    status="$(req POST /sandboxes "{\"tags\":{\"suite\":\"integration\",\"case\":\"image\"},\"image_id\":\"$IMG_ID\"}")"
    assert_status "create sandbox from the registered image" 200 "$status"
    SBX_IMG="$(extract id < "$WORKDIR/resp.json")"
    if [ -z "$SBX_IMG" ]; then
      fail "create sandbox from image returned no id — aborting remaining image checks"
    else
      CREATED_SANDBOXES+=("$SBX_IMG")
      pass "create sandbox from image returned an id ($SBX_IMG)"

      status="$(req POST "/sandboxes/$SBX_IMG/exec" '{"command":"echo","args":["booted-from-registered-image"]}')"
      assert_status "exec in the image-booted sandbox" 200 "$status"
      assert_contains "exec output is correct — the agent baked into the reused rootfs actually answered" \
        "$(cat "$WORKDIR/resp.json")" "booted-from-registered-image"

      status="$(req GET /images)"
      assert_contains "image listing reports the sandbox as the current holder" \
        "$(cat "$WORKDIR/resp.json")" "\"in_use_by\":\"sandbox $SBX_IMG\""

      status="$(req DELETE "/images/$IMG_ID")"
      assert_status "deleting an image while a sandbox references it is a conflict" 409 "$status"

      status="$(req DELETE "/sandboxes/$SBX_IMG")"
      assert_status "stop the image-booted sandbox" 204 "$status"
      CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_IMG}")

      status="$(req DELETE "/images/$IMG_ID")"
      assert_status "delete the image now that nothing references it" 204 "$status"
      CREATED_IMAGES=("${CREATED_IMAGES[@]/$IMG_ID}")
    fi
  fi
fi

status="$(req POST /sandboxes '{"image_id":"not-a-real-registered-image"}')"
assert_status "create sandbox with a nonexistent image_id is 404" 404 "$status"

status="$(req DELETE "/images/not-a-real-registered-image")"
assert_status "delete a nonexistent image is 404" 404 "$status"

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

    status="$(req GET "/snapshots?source_sandbox_id=$SBX4")"
    assert_status "list snapshots filtered by source_sandbox_id" 200 "$status"
    assert_contains "source_sandbox_id filter finds the snapshot the sandbox became" "$(cat "$WORKDIR/resp.json")" "$SNAP"

    status="$(req GET "/snapshots?source_sandbox_id=not-a-real-sandbox-id")"
    assert_status "list snapshots with a non-matching source_sandbox_id" 200 "$status"
    assert_contains "non-matching source_sandbox_id filter returns an empty list" "$(cat "$WORKDIR/resp.json")" '"snapshots":[]'

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

      status="$(req DELETE "/sandboxes/$SBX5?keep=false")"
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

  status="$(req DELETE "/sandboxes/$SBXP?keep=false")"
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
    assert_status "stopping a jailed sandbox with the persist-by-default behavior is a conflict (can't be snapshotted)" 409 "$status"
    assert_contains "the conflict message points at the ?keep=false opt-out" "$(cat "$WORKDIR/resp.json")" "keep=false"

    status="$(req DELETE "/sandboxes/$SBX_JAIL?keep=false")"
    assert_status "stop the jailed sandbox with the explicit destroy opt-out" 204 "$status"
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
      assert_status "stopping a forked sandbox with the default (persist) behavior still succeeds" 200 "$status"
      assert_contains "a fork has nothing new to preserve, so it's silently destroyed rather than erroring" "$(cat "$WORKDIR/resp.json")" '"kept":false'
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

        status="$(req DELETE "/sandboxes/$FORK2?keep=false")"
        assert_status "stop the second forked sandbox" 204 "$status"
        CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK2}")
      fi

      status="$(req POST "/snapshots/$FSNAP/resume")"
      assert_status "the snapshot can still be consumed via resume once no fork is live" 200 "$status"
      FORK3="$(extract id < "$WORKDIR/resp.json")"
      CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/$FSNAP}")  # resuming consumes the snapshot

      if [ -n "$FORK3" ]; then
        CREATED_SANDBOXES+=("$FORK3")
        status="$(req DELETE "/sandboxes/$FORK3?keep=false")"
        assert_status "stop the finally-resumed sandbox" 204 "$status"
        CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK3}")
      fi
    fi
  fi
fi

status="$(req POST /snapshots/not-a-real-snapshot/fork)"
assert_status "forking a nonexistent snapshot is 404" 404 "$status"

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

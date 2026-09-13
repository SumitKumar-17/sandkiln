section "remote storage mounts"

# Needs a real S3-compatible endpoint reachable from the guest's bridge
# network, plus a rootfs with rclone/fusermount3 injected and a
# CONFIG_FUSE_FS guest kernel (see SELF_HOSTING.md's "Remote storage
# mounts (optional)" section) -- not part of a normal daemon setup, so
# this whole topic is opt-in, same pattern as 10-jailer.sh's
# SANDKILN_JAILER_ENABLED gate.
if [ -z "${SANDKILN_MOUNTS_TEST_ENDPOINT:-}" ]; then
  echo "  skip - SANDKILN_MOUNTS_TEST_ENDPOINT not set, presumed no FUSE-capable test setup available"
else
  TEST_BUCKET="${SANDKILN_MOUNTS_TEST_BUCKET:-my-bucket}"
  TEST_ACCESS_KEY="${SANDKILN_MOUNTS_TEST_ACCESS_KEY:-testkey}"
  TEST_SECRET_KEY="${SANDKILN_MOUNTS_TEST_SECRET_KEY:-testsecret}"

  status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"mounts"}}')"
  assert_status "create sandbox" 200 "$status"
  SBX_MOUNT="$(extract id < "$WORKDIR/resp.json")"
  if [ -z "$SBX_MOUNT" ]; then
    fail "create sandbox for mounts returned no id -- aborting mounts checks"
  else
    CREATED_SANDBOXES+=("$SBX_MOUNT")

    body="$(printf '{"bucket":"%s","endpoint":"%s","access_key":"%s","secret_key":"%s","mount_path":"/mnt/data","read_only":false}' \
      "$TEST_BUCKET" "$SANDKILN_MOUNTS_TEST_ENDPOINT" "$TEST_ACCESS_KEY" "$TEST_SECRET_KEY")"
    status="$(req POST "/sandboxes/$SBX_MOUNT/mounts" "$body")"
    assert_status "mount the test bucket" 200 "$status"
    MOUNT_ID="$(extract id < "$WORKDIR/resp.json")"

    if [ -z "$MOUNT_ID" ]; then
      fail "mounting the test bucket returned no id -- aborting remaining mounts checks"
    else
      status="$(req GET "/sandboxes/$SBX_MOUNT/mounts")"
      assert_status "list mounts" 200 "$status"
      assert_contains "the new mount is listed" "$(cat "$WORKDIR/resp.json")" "$MOUNT_ID"

      status="$(req POST "/sandboxes/$SBX_MOUNT/mounts" "$body")"
      assert_status "mounting the same path again is a conflict" 409 "$status"

      status="$(req POST "/sandboxes/$SBX_MOUNT/exec" '{"command":"sh","args":["-c","echo integration-test-roundtrip > /mnt/data/integration-test.txt"]}')"
      assert_status "write a file through the mount" 200 "$status"

      status="$(req POST "/sandboxes/$SBX_MOUNT/exec" '{"command":"cat","args":["/mnt/data/integration-test.txt"]}')"
      assert_status "read the file back through the mount" 200 "$status"
      assert_contains "the written content round-trips" "$(cat "$WORKDIR/resp.json")" "integration-test-roundtrip"

      status="$(req DELETE "/sandboxes/$SBX_MOUNT/mounts/$MOUNT_ID")"
      assert_status "unmount" 204 "$status"

      status="$(req GET "/sandboxes/$SBX_MOUNT/mounts")"
      assert_status "list mounts after unmount" 200 "$status"
      assert_contains "the mount is no longer listed" "$(cat "$WORKDIR/resp.json")" '"mounts":[]'

      status="$(req POST "/sandboxes/$SBX_MOUNT/exec" '{"command":"mountpoint","args":["-q","/mnt/data"]}')"
      exit_code="$(jq -r '.exit_code // empty' < "$WORKDIR/resp.json")"
      if [ "$exit_code" != "0" ]; then
        pass "the mount point is actually unmounted"
      else
        fail "the mount point still reports as mounted after DELETE"
      fi
    fi
  fi

  status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"mounts-validation"}}')"
  assert_status "create a second sandbox for validation checks" 200 "$status"
  SBX_MOUNT_VALIDATION="$(extract id < "$WORKDIR/resp.json")"
  if [ -n "$SBX_MOUNT_VALIDATION" ]; then
    CREATED_SANDBOXES+=("$SBX_MOUNT_VALIDATION")
    status="$(req POST "/sandboxes/$SBX_MOUNT_VALIDATION/mounts" '{"bucket":"","endpoint":"http://example.com","access_key":"a","secret_key":"b","mount_path":"/mnt/data"}')"
    assert_status "an empty bucket is rejected" 400 "$status"
  fi
fi

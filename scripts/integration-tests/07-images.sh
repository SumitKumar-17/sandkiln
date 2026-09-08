section "images: register, boot a sandbox from it, in-use deletion refusal"
# Reuses the daemon's own currently-configured base rootfs (copied to a
# second path) as the "custom" image under test — building or fetching a
# genuinely distinct second image isn't practical for a scripted run, and
# the daemon can't tell the difference: image_id just changes which file
# create_sandbox clones from. Read from this script's own environment,
# same convention as the SANDKILN_JAILER_ENABLED/SANDKILN_AUTH_TOKEN
# checks elsewhere in this suite — expanded the same way
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

      status="$(req DELETE "/sandboxes/$SBX_IMG?keep=false")"
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

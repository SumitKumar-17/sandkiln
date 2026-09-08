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

  status="$(req DELETE "/sandboxes/$SBX_SEED?keep=false")"
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

  status="$(req DELETE "/sandboxes/$SBX_RO1?keep=false")"
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

  status="$(req DELETE "/sandboxes/$SBX_RO2?keep=false")"
  assert_status "stop the second read-only holder" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RO2}")

  status="$(req DELETE "/sandboxes/$SBX_RO3?keep=false")"
  assert_status "stop the third read-only holder" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RO3}")

  # Once every read-only holder is stopped, the drive must correctly
  # become available again immediately — no stale holder left behind by
  # a force-stop's teardown path.
  status="$(req POST /sandboxes "{\"drives\":[{\"id\":\"$DRV_RO\",\"read_only\":true}]}")"
  assert_status "a fresh read-only attach succeeds once every prior holder is stopped" 200 "$status"
  SBX_RO4="$(extract id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES+=("$SBX_RO4")

  status="$(req DELETE "/sandboxes/$SBX_RO4?keep=false")"
  assert_status "stop the final read-only holder" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RO4}")

  status="$(req DELETE "/drives/$DRV_RO")"
  assert_status "delete the read-only-shared drive now that nothing holds it" 204 "$status"
  CREATED_DRIVES=("${CREATED_DRIVES[@]/$DRV_RO}")
fi

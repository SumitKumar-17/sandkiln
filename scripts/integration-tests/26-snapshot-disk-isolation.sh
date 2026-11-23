# Regression coverage for a data-corruption bug found in the 2026-11 audit:
# a snapshot records its rootfs by host path, and a VM resumed from it used to
# write straight into that file. A fork (or a retain-history resume, or a
# time-travel restore) therefore modified the snapshot's own disk, and the
# next fork/resume read a filesystem that no longer matched the memory it was
# captured with (ext4 "Bad message"). Resume now runs on a private clone.
#
# The guest's cached metadata can hide the damage, so the second VM drops its
# caches before reading: that is what surfaced the original bug.
section "snapshot disk isolation across fork, resume and restore"

guest() { # guest <sandbox-id> <shell command>: prints combined stdout of an exec
  local body
  body="$(printf '{"command":"sh","args":["-c","%s"]}' "$2")"
  req POST "/sandboxes/$1/exec" "$body" >/dev/null
  cat "$WORKDIR/resp.json"
}

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"disk-isolation"}}')"
assert_status "create the source sandbox" 200 "$status"
SRC="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SRC" ]; then
  fail "no source sandbox id; aborting disk-isolation checks"
else
  CREATED_SANDBOXES+=("$SRC")
  guest "$SRC" 'echo base > /root/base-marker; sync' >/dev/null
  status="$(req POST "/sandboxes/$SRC/snapshot")"
  assert_status "snapshot the source sandbox" 200 "$status"
  SNAP="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SRC}")

  # ---- fork vs fork
  status="$(req POST "/snapshots/$SNAP/fork")"
  assert_status "fork the snapshot" 200 "$status"
  F1="$(extract id < "$WORKDIR/resp.json")"
  guest "$F1" 'echo from-fork1 > /root/fork1-marker; sync; sync' >/dev/null
  req DELETE "/sandboxes/$F1?keep=false" >/dev/null

  status="$(req POST "/snapshots/$SNAP/fork")"
  assert_status "fork the same snapshot again" 200 "$status"
  F2="$(extract id < "$WORKDIR/resp.json")"
  out="$(guest "$F2" 'echo 3 > /proc/sys/vm/drop_caches; cat /root/fork1-marker 2>&1')"
  assert_not_contains "a second fork does not see the first fork's write" "$out" "from-fork1"
  assert_not_contains "the second fork reads a consistent filesystem (no ext4 'Bad message')" "$out" "Bad message"
  assert_contains "the second fork still sees the snapshot's own data" "$(guest "$F2" 'cat /root/base-marker')" "base"
  req DELETE "/sandboxes/$F2?keep=false" >/dev/null

  # ---- retain-history resume, then restore the checkpoint twice
  status="$(req POST "/snapshots/$SNAP/resume")"
  assert_status "resume the snapshot, retaining history" 200 "$status"
  R1="$(extract id < "$WORKDIR/resp.json")"
  guest "$R1" 'echo from-resumed > /root/resumed-marker; sync; sync' >/dev/null
  req DELETE "/sandboxes/$R1?keep=false" >/dev/null

  status="$(req POST "/snapshots/history/$SNAP/restore")"
  assert_status "restore the retired checkpoint" 200 "$status"
  X1="$(extract id < "$WORKDIR/resp.json")"
  out="$(guest "$X1" 'echo 3 > /proc/sys/vm/drop_caches; cat /root/resumed-marker 2>&1')"
  assert_not_contains "a restore does not see writes the resumed sandbox made after the checkpoint" "$out" "from-resumed"
  guest "$X1" 'echo from-restore1 > /root/restore1-marker; sync; sync' >/dev/null
  req DELETE "/sandboxes/$X1?keep=false" >/dev/null

  status="$(req POST "/snapshots/history/$SNAP/restore")"
  assert_status "restore the same checkpoint a second time" 200 "$status"
  X2="$(extract id < "$WORKDIR/resp.json")"
  out="$(guest "$X2" 'echo 3 > /proc/sys/vm/drop_caches; cat /root/restore1-marker 2>&1')"
  assert_not_contains "a second restore does not see the first restore's write" "$out" "from-restore1"
  assert_not_contains "the second restore reads a consistent filesystem" "$out" "Bad message"
  req DELETE "/sandboxes/$X2?keep=false" >/dev/null
  req DELETE "/snapshots/history/$SNAP" >/dev/null
  req DELETE "/snapshots/$SNAP" >/dev/null
fi

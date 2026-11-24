# A snapshot of a cold-booted sandbox writes only the pages the guest dirtied
# since boot (Firecracker diff snapshot, `snapshot_dirty_tracking`), leaving a
# sparse memory file; a resumed sandbox is snapshotted in full. Either way the
# restored guest must see exactly the memory it had. A page the dirty tracking
# missed would come back as zeros, so the checks read data that lives only in
# guest memory: a tmpfs file written from random bytes, and the page cache a
# batch of reads from the root disk filled (pages the virtio block device wrote,
# not the guest CPU, the case dirty tracking is most likely to get wrong).
#
# Shape 256 MiB, not the default, so a pool configured by another topic cannot
# claim these creates. Compares per-sandbox facts and metric counts that only
# ever grow, never a global total, because other topics snapshot concurrently.
section "snapshot memory integrity across diff and full takes"

guest_out() { # guest_out <sandbox-id> <shell command>: prints the exec's stdout
  local body
  body="$(jq -nc --arg c "$2" '{command:"sh",args:["-c",$c]}')"
  req POST "/sandboxes/$1/exec" "$body" >/dev/null
  jq -r '.stdout // empty' < "$WORKDIR/resp.json"
}

# Counts the files whose checksum no longer matches; "ok" and "0" mean intact.
VERIFY='md5sum -c /dev/shm/blob.md5 2>&1 | tail -1; md5sum -c /dev/shm/files.md5 2>&1 | grep -vc ": OK"'

verify_guest() { # verify_guest <label> <sandbox-id>
  local out
  out="$(guest_out "$2" "$VERIFY")"
  assert_contains "$1: the tmpfs data is byte-identical" "$out" "/dev/shm/blob: OK"
  assert_eq "$1: no page-cache-backed file reads back different" "0" "$(tail -1 <<<"$out")"
}

phase_count() { # phase_count <phase>: the snapshot_take_phase_duration_ms count so far
  curl -s "${AUTH_HEADER[@]}" "$BASE_URL/metrics" | sed -n "s/^snapshot_take_phase_duration_ms_count{phase=\"$1\"} //p"
}

status="$(req POST /sandboxes '{"mem_size_mib":256,"tags":{"suite":"integration","case":"snapshot-memory-integrity"}}')"
assert_status "create the source sandbox" 200 "$status"
SRC="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SRC" ]; then
  fail "no source sandbox id; aborting snapshot memory integrity checks"
else
  CREATED_SANDBOXES+=("$SRC")
  guest_out "$SRC" 'head -c 64M /dev/urandom > /dev/shm/blob; md5sum /dev/shm/blob > /dev/shm/blob.md5; find /usr /lib -xdev -type f -size -1M 2>/dev/null | head -1500 | xargs md5sum > /dev/shm/files.md5 2>/dev/null; wc -l < /dev/shm/files.md5' >/dev/null
  verify_guest "before any snapshot" "$SRC"

  created_before="$(phase_count create)"
  status="$(req POST "/sandboxes/$SRC/snapshot")"
  assert_status "snapshot the cold-booted sandbox" 200 "$status"
  SNAP1="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SRC}")
  CREATED_SNAPSHOTS+=("$SNAP1")

  req GET /snapshots >/dev/null
  assert_contains "the snapshot is listed" "$(jq -r '.snapshots[].id' < "$WORKDIR/resp.json")" "$SNAP1"
  created_after="$(phase_count create)"
  if [ -n "$created_before" ] && [ -n "$created_after" ] && [ "$created_after" -gt "$created_before" ]; then
    pass "the take was recorded in snapshot_take_phase_duration_ms (create $created_before -> $created_after)"
  else
    fail "snapshot_take_phase_duration_ms{phase=\"create\"} did not grow ('$created_before' -> '$created_after')"
  fi

  status="$(req POST "/snapshots/$SNAP1/fork")"
  assert_status "fork the snapshot" 200 "$status"
  FORK="$(extract id < "$WORKDIR/resp.json")"
  if [ -n "$FORK" ]; then
    CREATED_SANDBOXES+=("$FORK")
    verify_guest "a fork of the diff snapshot" "$FORK"
    req DELETE "/sandboxes/$FORK?keep=false" >/dev/null
  fi

  status="$(req POST "/snapshots/$SNAP1/resume")"
  assert_status "resume the snapshot" 200 "$status"
  RES="$(extract id < "$WORKDIR/resp.json")"
  if [ -z "$RES" ]; then
    fail "no resumed sandbox id; aborting the remaining integrity checks"
  else
    CREATED_SANDBOXES+=("$RES")
    verify_guest "the resumed sandbox" "$RES"

    # A resumed VM has no dirty tracking (its memory came from a file), so this
    # take is a full one; it must round-trip the same data.
    status="$(req POST "/sandboxes/$RES/snapshot")"
    assert_status "snapshot the resumed sandbox (full take)" 200 "$status"
    SNAP2="$(extract snapshot_id < "$WORKDIR/resp.json")"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$RES}")
    CREATED_SNAPSHOTS+=("$SNAP2")

    status="$(req POST "/snapshots/$SNAP2/resume")"
    assert_status "resume the full snapshot of a resumed sandbox" 200 "$status"
    RES2="$(extract id < "$WORKDIR/resp.json")"
    if [ -n "$RES2" ]; then
      CREATED_SANDBOXES+=("$RES2")
      verify_guest "the resumed sandbox of a full take" "$RES2"
      req DELETE "/sandboxes/$RES2?keep=false" >/dev/null
    fi
  fi
  req DELETE "/snapshots/$SNAP1" >/dev/null
  req DELETE "/snapshots/$SNAP2" >/dev/null
fi

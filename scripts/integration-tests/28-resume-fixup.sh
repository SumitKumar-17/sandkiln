# A restored guest wakes with the entropy pool and clock it had at snapshot
# time. The daemon sends a fixup after every resume and fork (ResumeFixup),
# so clones of one snapshot must not share random bytes, and a clone resumed
# long after the snapshot must have the host's current time, not a stale one.
section "per-clone entropy and clock after resume"

guest() { # guest <sandbox-id> <shell command>: prints the exec response body
  local body
  body="$(printf '{"command":"sh","args":["-c","%s"]}' "$2")"
  req POST "/sandboxes/$1/exec" "$body" >/dev/null
  cat "$WORKDIR/resp.json"
}
stdout_of() { python3 -c 'import sys,json; print(json.load(sys.stdin)["stdout"].strip())'; }

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"resume-fixup"}}')"
assert_status "create the source sandbox" 200 "$status"
SRC="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SRC" ]; then
  fail "no source sandbox id; aborting resume-fixup checks"
else
  CREATED_SANDBOXES+=("$SRC")
  status="$(req POST "/sandboxes/$SRC/snapshot")"
  assert_status "snapshot the source sandbox" 200 "$status"
  SNAP="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SRC}")
  # Let the snapshot age so a stale clock is clearly distinguishable.
  sleep 5

  status="$(req POST "/snapshots/$SNAP/fork")"
  assert_status "fork the snapshot" 200 "$status"
  F1="$(extract id < "$WORKDIR/resp.json")"
  rand1="$(guest "$F1" 'head -c 16 /dev/urandom | sha256sum' | stdout_of)"
  uuid1="$(guest "$F1" 'cat /proc/sys/kernel/random/uuid' | stdout_of)"
  guest_now="$(guest "$F1" 'date +%s' | stdout_of)"
  host_now="$(date +%s)"
  skew=$(( host_now - guest_now )); [ "$skew" -lt 0 ] && skew=$(( -skew ))
  if [ "$skew" -le 2 ]; then pass "the clone's clock matches the host within 2s (skew ${skew}s after a 5s pause)"; else fail "the clone's clock is ${skew}s off the host: it resumed with a stale clock"; fi
  req DELETE "/sandboxes/$F1?keep=false" >/dev/null

  status="$(req POST "/snapshots/$SNAP/fork")"
  assert_status "fork the same snapshot again" 200 "$status"
  F2="$(extract id < "$WORKDIR/resp.json")"
  rand2="$(guest "$F2" 'head -c 16 /dev/urandom | sha256sum' | stdout_of)"
  uuid2="$(guest "$F2" 'cat /proc/sys/kernel/random/uuid' | stdout_of)"
  req DELETE "/sandboxes/$F2?keep=false" >/dev/null

  if [ -n "$rand1" ] && [ "$rand1" != "$rand2" ]; then pass "two forks of one snapshot read different /dev/urandom bytes"; else fail "two forks of one snapshot read the same /dev/urandom bytes ($rand1)"; fi
  if [ -n "$uuid1" ] && [ "$uuid1" != "$uuid2" ]; then pass "two forks of one snapshot generate different kernel UUIDs"; else fail "two forks generated the same kernel UUID ($uuid1)"; fi
fi

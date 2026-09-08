section "pre-warmed pools"
# Waits for real replenishment (a real cold boot + pause + snapshot +
# stop, see crate::pool_replenisher) to happen against the live daemon --
# no mocking the timing. A pool's replenish interval is a fixed 2s (see
# pool_replenisher::CHECK_INTERVAL), so this polls rather than assuming a
# fixed sleep is enough.
wait_for_warm() {
  local pool_id="$1" want="$2" tries=0 ready
  while [ "$tries" -lt 15 ]; do
    req GET /pools >/dev/null
    # `extract` only handles a plain top-level field, and this needs to
    # find one pool by id within a list -- read it directly with jq/grep
    # here instead of going through that helper.
    if command -v jq >/dev/null 2>&1; then
      ready="$(jq -r ".pools[] | select(.id==\"$pool_id\") | .warm_ready" < "$WORKDIR/resp.json")"
    else
      ready="$(grep -o "\"id\":\"$pool_id\"[^}]*\"warm_ready\":[0-9]*" "$WORKDIR/resp.json" | grep -o '[0-9]*$')"
    fi
    [ "${ready:-0}" -ge "$want" ] && return 0
    tries=$((tries + 1))
    sleep 2
  done
  return 1
}

status="$(req POST /pools "{\"id\":\"it-pool-$$\",\"warm_count\":1}")"
assert_status "create a pool" 200 "$status"
CREATED_POOLS+=("it-pool-$$")

if wait_for_warm "it-pool-$$" 1; then
  pass "pool replenished a warm snapshot in the background"
else
  fail "pool never became warm within 30s"
fi

status="$(req GET /pools)"
assert_status "list pools" 200 "$status"
pools_body="$(cat "$WORKDIR/resp.json")"
assert_contains "pool list includes the configured pool" "$pools_body" "it-pool-$$"

# A plain create with no drives/rate_limit and the pool's own (default)
# resource config matches and claims -- fast enough to distinguish from a
# cold create, though this doesn't assert a specific number (see
# ROADMAP.md's Benchmarking section for real measured numbers; a
# shared, variable-load dev box makes a hard latency assertion flaky).
status="$(req POST /sandboxes "{\"name\":\"it-pool-claim-$$\",\"tags\":{\"suite\":\"integration\",\"case\":\"pool\"}}")"
assert_status "claim a sandbox from the pool" 200 "$status"
CLAIMED_SBX="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$CLAIMED_SBX" ]; then
  fail "claimed sandbox returned no id — aborting remaining pool checks"
else
  CREATED_SANDBOXES+=("$CLAIMED_SBX")

  status="$(req GET /pools)"
  pool_after_claim="$(cat "$WORKDIR/resp.json")"
  assert_contains "warm_ready dropped after the claim" "$pool_after_claim" "\"warm_ready\":0"

  # The claimed sandbox must carry the CALLER's real name/tags, not
  # pool_replenisher's placeholder ones -- the whole point of overwriting
  # identity post-resume in claim_from_pool.
  status="$(req GET /sandboxes)"
  list_body="$(cat "$WORKDIR/resp.json")"
  assert_contains "claimed sandbox's real name is visible in the sandbox list" "$list_body" "it-pool-claim-$$"

  status="$(req POST "/sandboxes/$CLAIMED_SBX/exec" '{"command":"echo","args":["pool-claimed-sandbox-is-alive"]}')"
  assert_status "claimed sandbox actually responds to exec (the post-resume health check passed)" 200 "$status"
  assert_contains "exec output is real, not stale" "$(cat "$WORKDIR/resp.json")" "pool-claimed-sandbox-is-alive"

  # MMDS must reflect the caller's real identity too -- see
  # sandkiln_vmm::vm::Vm::update_metadata's doc comment for why a resumed
  # VM needs a full PUT /mmds/config + PUT /mmds, not a bare PATCH.
  # Retried briefly: this exec-over-vsock check above already proved the
  # resumed guest is up, but MMDS rides the guest's separate network
  # readiness path, which has its own brief settling window right after a
  # resume, especially under this dev box's own concurrent load from
  # other tests/replenishment running at the same time.
  mmds_ok=""
  for _ in $(seq 1 8); do
    status="$(req POST "/sandboxes/$CLAIMED_SBX/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"TOKEN=\$(curl -s -X PUT http://169.254.169.254/latest/api/token -H \\\"X-metadata-token-ttl-seconds: 21600\\\") && curl -s -H \\\"X-metadata-token: \$TOKEN\\\" -H \\\"Accept: application/json\\\" http://169.254.169.254/\"]}")"
    mmds_body="$(cat "$WORKDIR/resp.json")"
    case "$mmds_body" in
      *"it-pool-claim-$$"*) mmds_ok="yes"; break ;;
    esac
    sleep 1
  done
  assert_status "exec fetching MMDS content from the claimed sandbox" 200 "$status"
  assert_eq "MMDS content reflects the claimed sandbox's real name, not the pool's placeholder" "yes" "${mmds_ok:-no (last response: $mmds_body)}"

  status="$(req DELETE "/sandboxes/$CLAIMED_SBX?keep=false")"
  assert_status "stop the claimed sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$CLAIMED_SBX}")
fi

# A create with drives, or a custom rate_limit, must never match a pool
# (both are baked into a VM's boot-time state, which a warm snapshot
# never has) -- it should still succeed via a normal cold create, not
# error, and must leave the pool's own warm snapshot untouched.
if wait_for_warm "it-pool-$$" 1; then
  status="$(req POST /sandboxes "{\"rate_limit\":{\"bandwidth_bytes_per_sec\":1000000}}")"
  assert_status "a create with a custom rate_limit bypasses the pool and still succeeds" 200 "$status"
  BYPASS_SBX="$(extract id < "$WORKDIR/resp.json")"
  [ -n "$BYPASS_SBX" ] && CREATED_SANDBOXES+=("$BYPASS_SBX")

  status="$(req GET /pools)"
  pool_after_bypass="$(cat "$WORKDIR/resp.json")"
  assert_contains "the pool's warm snapshot is untouched by a request it can't match" "$pool_after_bypass" "\"warm_ready\":1"

  if [ -n "$BYPASS_SBX" ]; then
    status="$(req DELETE "/sandboxes/$BYPASS_SBX?keep=false")"
    assert_status "stop the rate-limited (pool-bypassing) sandbox" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$BYPASS_SBX}")
  fi
else
  fail "pool never re-warmed after the first claim within 30s — skipping the bypass check"
fi

# Deleting a pool must clean up whatever it still has warm, not leak it
# as an orphaned, untracked snapshot on disk.
status="$(req GET /snapshots)"
snapshots_before_delete="$(cat "$WORKDIR/resp.json")"
snapshot_count_before="$(echo "$snapshots_before_delete" | grep -o '"id"' | wc -l)"

status="$(req DELETE "/pools/it-pool-$$")"
assert_status "delete the pool" 204 "$status"
CREATED_POOLS=("${CREATED_POOLS[@]/it-pool-$$}")

status="$(req GET /pools)"
assert_not_contains "deleted pool no longer appears in the pool list" "$(cat "$WORKDIR/resp.json")" "it-pool-$$"

status="$(req GET /snapshots)"
snapshot_count_after="$(cat "$WORKDIR/resp.json" | grep -o '"id"' | wc -l)"
assert_eq "deleting the pool also cleaned up its warm snapshot" "$((snapshot_count_before - 1))" "$snapshot_count_after"

status="$(req DELETE "/pools/it-pool-$$")"
assert_status "deleting an already-deleted pool is a 404" 404 "$status"

section "pre-warmed pools: max_count ceiling and queueing"
# warm_count: 0 here deliberately -- this section is only about the
# max_count/claimed-count lifecycle, not replenishment (already covered
# above), so every claim below cold-creates under capacity rather than
# resuming anything.
status="$(req POST /pools "{\"id\":\"it-pool-max-$$\",\"warm_count\":0,\"max_count\":1}")"
assert_status "create a pool with max_count: 1" 200 "$status"
CREATED_POOLS+=("it-pool-max-$$")

pool_claimed() {
  req GET /pools >/dev/null
  if command -v jq >/dev/null 2>&1; then
    jq -r ".pools[] | select(.id==\"it-pool-max-$$\") | .claimed" < "$WORKDIR/resp.json"
  else
    grep -o "\"id\":\"it-pool-max-$$\"[^}]*\"claimed\":[0-9]*" "$WORKDIR/resp.json" | grep -o '[0-9]*$'
  fi
}

status="$(req POST /sandboxes "{\"name\":\"it-pool-max-1-$$\"}")"
assert_status "first claim succeeds and cold-creates under max_count headroom" 200 "$status"
SBX_MAX_1="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_MAX_1" ]; then
  fail "first max_count claim returned no id — aborting max_count checks"
else
  CREATED_SANDBOXES+=("$SBX_MAX_1")
  assert_eq "pool reports 1 claimed instance after the first create" "1" "$(pool_claimed)"

  # A second concurrent claim has nothing warm and no room (max_count: 1,
  # already at capacity) -- it must queue rather than reject outright or
  # silently exceed the ceiling. Backgrounded so this script can confirm
  # it's still pending, then free the slot and confirm it completes
  # promptly afterward (a real, event-driven wakeup, not a fixed poll
  # interval -- see Pool::notify's doc comment). A raw curl of its own,
  # not the shared `req` helper -- `req` always writes to the one shared
  # $WORKDIR/resp.json, which this script's own foreground calls (the
  # DELETE below, then more `req` calls afterward) would race and
  # clobber while this is still in flight.
  curl -s -o "$WORKDIR/pool-max-2-resp.json" -X POST "$BASE_URL/sandboxes" \
    -H 'Content-Type: application/json' "${AUTH_HEADER[@]}" -d "{\"name\":\"it-pool-max-2-$$\"}" &
  QUEUED_PID=$!
  sleep 4
  if kill -0 "$QUEUED_PID" 2>/dev/null; then
    pass "a second claim at max_count queues instead of rejecting or exceeding the ceiling"
  else
    fail "a second claim at max_count returned immediately -- expected it to queue"
  fi

  status="$(req DELETE "/sandboxes/$SBX_MAX_1?keep=false")"
  assert_status "stop the first max_count sandbox, freeing its slot" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_MAX_1}")

  # The queued claim should wake up promptly once the slot frees -- not
  # wait out its own ~30s queue timeout. A generous but bounded wait,
  # not a magic-number sleep tuned to pass by luck.
  wait_ok=""
  for _ in $(seq 1 15); do
    kill -0 "$QUEUED_PID" 2>/dev/null || { wait_ok="yes"; break; }
    sleep 1
  done
  wait "$QUEUED_PID" 2>/dev/null
  if [ "$wait_ok" = "yes" ]; then
    pass "the queued claim woke up and completed promptly once a slot freed, not after the full queue timeout"
  else
    fail "the queued claim did not complete within 15s of a slot freeing up"
  fi

  SBX_MAX_2="$(extract id < "$WORKDIR/pool-max-2-resp.json")"
  if [ -z "$SBX_MAX_2" ]; then
    fail "queued claim produced no sandbox id ($(cat "$WORKDIR/pool-max-2-resp.json"))"
  else
    CREATED_SANDBOXES+=("$SBX_MAX_2")
    assert_eq "pool reports 1 claimed instance again after the queued claim completed" "1" "$(pool_claimed)"

    status="$(req DELETE "/sandboxes/$SBX_MAX_2?keep=false")"
    assert_status "stop the second max_count sandbox" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_MAX_2}")
    assert_eq "pool reports 0 claimed instances once both are stopped" "0" "$(pool_claimed)"
  fi
fi

# The full ~30s queue-timeout-then-503 path is deliberately not exercised
# here -- it would add a mandatory ~30s to every run of this suite for a
# path already verified manually against a live daemon (a held sandbox
# with nothing freeing its pool's only slot; the second claim returned a
# real 503 with a clear message after exactly 30.0s). Same tradeoff this
# suite already makes for the sqlite-history restart case (see
# 16-sandbox-history.sh) -- not everything worth verifying once needs to
# cost every future run.
status="$(req DELETE "/pools/it-pool-max-$$")"
assert_status "delete the max_count pool" 204 "$status"
CREATED_POOLS=("${CREATED_POOLS[@]/it-pool-max-$$}")

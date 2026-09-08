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
  for _ in 1 2 3 4 5; do
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

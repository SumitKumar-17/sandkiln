section "per-sandbox I/O rate limiting"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"rate-limit"},"rate_limit":{"bandwidth_bytes_per_sec":5000000,"ops_per_sec":1000}}')"
assert_status "create sandbox with a valid rate_limit" 200 "$status"
SBX_RL="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_RL" ]; then
  fail "create sandbox with rate_limit returned no id — aborting rate-limit checks"
else
  CREATED_SANDBOXES+=("$SBX_RL")
  pass "create sandbox with rate_limit returned an id ($SBX_RL) — Firecracker accepted the rate_limiter/rx_rate_limiter/tx_rate_limiter PUT bodies"

  status="$(req POST "/sandboxes/$SBX_RL/exec" '{"command":"echo","args":["rate-limited-ok"]}')"
  assert_status "exec in rate-limited sandbox still works" 200 "$status"
  assert_contains "exec output is correct" "$(cat "$WORKDIR/resp.json")" "rate-limited-ok"

  status="$(req DELETE "/sandboxes/$SBX_RL?keep=false")"
  assert_status "stop rate-limited sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_RL}")
fi

status="$(req POST /sandboxes '{"rate_limit":{}}')"
assert_status "rate_limit with neither bandwidth nor ops set is rejected" 400 "$status"

status="$(req POST /sandboxes '{"rate_limit":{"bandwidth_bytes_per_sec":0}}')"
assert_status "rate_limit.bandwidth_bytes_per_sec of 0 is rejected" 400 "$status"

status="$(req POST /sandboxes '{"rate_limit":{"ops_per_sec":0}}')"
assert_status "rate_limit.ops_per_sec of 0 is rejected" 400 "$status"

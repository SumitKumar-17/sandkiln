section "health and metrics"
status="$(req GET /healthz)"
assert_status "GET /healthz" 200 "$status"

status="$(req GET /metrics)"
assert_status "GET /metrics (unauthenticated, even with auth on)" 200 "$status"
metrics_body="$(cat "$WORKDIR/resp.json")"
assert_contains "metrics include sandboxes_created_total" "$metrics_body" "sandboxes_created_total"
assert_contains "metrics include sandboxes_active" "$metrics_body" "sandboxes_active"

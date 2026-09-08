section "request id correlation"
resp_headers="$(curl -s -D - -o /dev/null "$BASE_URL/healthz")"
assert_contains "a request with no X-Request-Id gets one generated and echoed back" "$resp_headers" "x-request-id:"

custom_request_id="integration-test-$(date +%s)-$$"
resp_headers="$(curl -s -D - -o /dev/null -H "X-Request-Id: $custom_request_id" "$BASE_URL/healthz")"
assert_contains "a caller-supplied X-Request-Id is echoed back verbatim" "$resp_headers" "$custom_request_id"

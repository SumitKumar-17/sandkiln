section "error cases"
status="$(req POST "/sandboxes/not-a-real-id/exec" '{"command":"echo","args":[]}')"
assert_status "exec against a nonexistent sandbox is 404" 404 "$status"

status="$(req DELETE "/sandboxes/not-a-real-id")"
assert_status "stop a nonexistent sandbox is 404" 404 "$status"

status="$(req DELETE "/drives/not-a-real-drive")"
assert_status "delete a nonexistent drive is 404" 404 "$status"

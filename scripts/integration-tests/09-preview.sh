section "dev server preview"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"preview"}}')"
assert_status "create sandbox for preview test" 200 "$status"
SBXP="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBXP" ]; then
  fail "create sandbox for preview test returned no id — aborting preview checks"
else
  CREATED_SANDBOXES+=("$SBXP")

  # A real dev server, backgrounded and detached from the exec call's own
  # stdout/stderr pipes (redirected to a log file instead) so `sh -c`
  # exits immediately once the server has forked, rather than blocking on
  # a pipe the backgrounded process would otherwise still be holding open.
  status="$(req POST "/sandboxes/$SBXP/exec" '{"command":"sh","args":["-c","cd /tmp && python3 -m http.server 8123 </dev/null >/tmp/preview-server.log 2>&1 & sleep 1"]}')"
  assert_status "start a dev server inside the sandbox" 200 "$status"

  status="$(req GET "/sandboxes/$SBXP/preview/8123/")"
  assert_status "preview proxies a real HTTP response from the guest" 200 "$status"
  assert_contains "preview response body is the guest server's own content" "$(cat "$WORKDIR/resp.json")" "Directory listing for /"

  status="$(req GET "/sandboxes/not-a-real-id/preview/8123/")"
  assert_status "preview against a nonexistent sandbox is 404" 404 "$status"

  status="$(req GET "/sandboxes/$SBXP/preview/59999/")"
  assert_status "preview against a port nothing is listening on is 502" 502 "$status"

  if [ -n "$AUTH_TOKEN" ]; then
    status="$(curl -s -o "$WORKDIR/resp.json" -w '%{http_code}' "$BASE_URL/sandboxes/$SBXP/preview/8123/")"
    assert_status "preview with no token at all is rejected" 401 "$status"

    status="$(curl -s -o "$WORKDIR/resp.json" -w '%{http_code}' "$BASE_URL/sandboxes/$SBXP/preview/8123/?token=$AUTH_TOKEN")"
    assert_status "preview accepts the token as a query parameter (no Authorization header)" 200 "$status"
  else
    echo "  skip - SANDKILN_AUTH_TOKEN not set, preview auth checks skipped (see the 'auth' section below)"
  fi

  status="$(req DELETE "/sandboxes/$SBXP?keep=false")"
  assert_status "stop the preview sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBXP}")
fi

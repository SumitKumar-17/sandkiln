section "auth"
if [ -z "$AUTH_TOKEN" ]; then
  echo "  skip - SANDKILN_AUTH_TOKEN not set, daemon presumably running without auth"
else
  saved_header=("${AUTH_HEADER[@]}")
  AUTH_HEADER=()
  status="$(req POST /sandboxes '{}')"
  assert_status "create without a token is rejected" 401 "$status"
  AUTH_HEADER=(-H "Authorization: Bearer wrong-token-entirely")
  status="$(req POST /sandboxes '{}')"
  assert_status "create with the wrong token is rejected" 401 "$status"
  AUTH_HEADER=("${saved_header[@]}")
  status="$(req POST /sandboxes '{}')"
  assert_status "create with the correct token succeeds" 200 "$status"
  SBX_AUTH="$(extract id < "$WORKDIR/resp.json")"
  [ -n "$SBX_AUTH" ] && CREATED_SANDBOXES+=("$SBX_AUTH")
fi

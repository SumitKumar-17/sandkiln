section "guest-accessible metadata (MMDS)"
# Firecracker's own MMDS (Microvm Metadata Service) at
# http://169.254.169.254/ inside the guest -- no sandkiln HTTP route or
# SDK method involved, this is entirely served by Firecracker's device
# model. Configured V2 (token-gated): the guest must PUT for a session
# token before it can GET anything. GETting a nested JSON path with no
# Accept header returns an AWS-IMDS-style newline-separated list of
# child key names (a real, sometimes-surprising Firecracker behavior,
# not a sandkiln bug) -- Accept: application/json returns the actual
# value.
status="$(req POST /sandboxes "{\"name\":\"it-mmds-$$\",\"tags\":{\"suite\":\"integration\",\"case\":\"mmds\"}}")"
assert_status "create sandbox with a name and tags for the MMDS test" 200 "$status"
SBX_MMDS="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_MMDS" ]; then
  fail "create sandbox for MMDS test returned no id — aborting MMDS checks"
else
  CREATED_SANDBOXES+=("$SBX_MMDS")

  status="$(req POST "/sandboxes/$SBX_MMDS/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"TOKEN=\$(curl -s -X PUT http://169.254.169.254/latest/api/token -H \\\"X-metadata-token-ttl-seconds: 21600\\\") && curl -s -H \\\"X-metadata-token: \$TOKEN\\\" -H \\\"Accept: application/json\\\" http://169.254.169.254/\"]}")"
  assert_status "exec fetching MMDS content (token flow + Accept: application/json)" 200 "$status"
  mmds_body="$(cat "$WORKDIR/resp.json")"
  assert_contains "MMDS content includes this sandbox's own id" "$mmds_body" "$SBX_MMDS"
  assert_contains "MMDS content includes the sandbox's name" "$mmds_body" "it-mmds-$$"
  assert_contains "MMDS content includes the sandbox's tags" "$mmds_body" 'suite'

  status="$(req DELETE "/sandboxes/$SBX_MMDS?keep=false")"
  assert_status "stop the MMDS-test sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_MMDS}")
fi

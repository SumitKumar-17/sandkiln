section "per-sandbox egress (outbound network) policy"

# Actual allow/deny/deny-wins-on-overlap behavior needs a real second
# reachable IP on the daemon host's own LAN to test against (the bridge
# gateway itself is *always* reachable regardless of policy, by design --
# see egress.rs's module doc comment -- so it can't be used to prove
# blocking), which this suite can't assume exists on every machine it
# runs on. That behavior was verified by hand against the real dev box
# instead (see ROADMAP.md's Firewall and egress policy entry). What's
# covered here, host-agnostically: request validation, that a policy
# doesn't break normal sandbox use, and that it survives snapshot/resume
# and fork without error.

status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"egress-deny-all"},"egress":{"mode":"deny_all","allow_cidrs":["10.0.0.0/8"]}}')"
assert_status "create sandbox with a deny_all + allow_cidrs policy" 200 "$status"
SBX_EG="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_EG" ]; then
  fail "create sandbox with egress policy returned no id -- aborting egress checks"
else
  CREATED_SANDBOXES+=("$SBX_EG")
  pass "create sandbox with egress policy returned an id ($SBX_EG) -- iptables chain/jump rule applied without error"

  status="$(req POST "/sandboxes/$SBX_EG/exec" '{"command":"echo","args":["egress-ok"]}')"
  assert_status "exec in an egress-policy sandbox still works" 200 "$status"
  assert_contains "exec output is correct" "$(cat "$WORKDIR/resp.json")" "egress-ok"

  status="$(req POST "/sandboxes/$SBX_EG/snapshot")"
  assert_status "snapshot a sandbox with an egress policy" 200 "$status"
  SNAP_EG="$(extract snapshot_id < "$WORKDIR/resp.json")"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_EG}")

  if [ -z "$SNAP_EG" ]; then
    fail "snapshotting an egress-policy sandbox returned no snapshot_id -- aborting resume/fork checks"
  else
    CREATED_SNAPSHOTS+=("$SNAP_EG")

    status="$(req POST "/snapshots/$SNAP_EG/fork")"
    assert_status "fork a snapshot with an egress policy re-applies it without error" 200 "$status"
    FORK_EG="$(extract id < "$WORKDIR/resp.json")"
    [ -n "$FORK_EG" ] && CREATED_SANDBOXES+=("$FORK_EG")

    status="$(req DELETE "/sandboxes/$FORK_EG?keep=false")"
    assert_status "destroy the fork removes its egress chain without error" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK_EG}")

    status="$(req POST "/snapshots/$SNAP_EG/resume")"
    assert_status "resume a snapshot with an egress policy re-applies it without error" 200 "$status"
    RESUME_EG="$(extract id < "$WORKDIR/resp.json")"
    [ -n "$RESUME_EG" ] && CREATED_SANDBOXES+=("$RESUME_EG")
  fi
fi

status="$(req POST /sandboxes '{"egress":{"mode":"allow_all","deny_cidrs":["not-a-cidr"]}}')"
assert_status "a malformed deny_cidrs entry is rejected" 400 "$status"
assert_contains "the error names the bad entry" "$(cat "$WORKDIR/resp.json")" "not-a-cidr"

status="$(req POST /sandboxes '{"egress":{"mode":"allow_all","allow_cidrs":["10.0.0.0/33"]}}')"
assert_status "a prefix length out of range is rejected" 400 "$status"

status="$(req POST /sandboxes '{"egress":{"mode":"deny_all"}}')"
assert_status "deny_all with no allow_cidrs at all is a valid (maximally restrictive) policy" 200 "$status"
SBX_EG_STRICT="$(extract id < "$WORKDIR/resp.json")"
[ -n "$SBX_EG_STRICT" ] && CREATED_SANDBOXES+=("$SBX_EG_STRICT")

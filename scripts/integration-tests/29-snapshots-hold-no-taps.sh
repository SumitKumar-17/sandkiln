# A held snapshot must not consume a tap device. A guest needs a tap only while
# it runs: its IP/MAC are frozen in the memory image and stay owned by the
# snapshot, but resume plugs it into whichever tap is free (Firecracker's
# network_overrides). Before this, every snapshot pinned a tap, so a pool of
# 32 allowed 32 snapshots and then every create failed.
#
# Compares exact id sets and per-sandbox facts, never the global tap count:
# other topics lease taps concurrently, so only "more snapshots than taps
# exist, and everything still works" is race-free.
section "held snapshots do not pin tap devices"

req GET /status >/dev/null
TAP_POOL_SIZE="$(jq -r '.tap_pool.size // empty' < "$WORKDIR/resp.json")"
if [ -z "$TAP_POOL_SIZE" ] || [ "$TAP_POOL_SIZE" -lt 1 ]; then
  echo "  skip - no tap pool configured, nothing to exhaust"
else
  # More snapshots than the pool has taps, so the old behaviour cannot pass.
  WANT=$((TAP_POOL_SIZE + 6))
  HELD=()
  failed_at=""
  for i in $(seq 1 "$WANT"); do
    status="$(req POST /sandboxes '{"mem_size_mib":128,"tags":{"suite":"integration","case":"snapshots-hold-no-taps"}}')"
    id="$(extract id < "$WORKDIR/resp.json")"
    if [ "$status" != 200 ] || [ -z "$id" ]; then failed_at="create #$i ($status: $(cat "$WORKDIR/resp.json"))"; break; fi
    CREATED_SANDBOXES+=("$id")
    status="$(req POST "/sandboxes/$id/snapshot")"
    snap="$(extract snapshot_id < "$WORKDIR/resp.json")"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$id}")
    if [ "$status" != 200 ] || [ -z "$snap" ]; then failed_at="snapshot #$i ($status: $(cat "$WORKDIR/resp.json"))"; break; fi
    CREATED_SNAPSHOTS+=("$snap")
    HELD+=("$snap")
  done
  if [ -z "$failed_at" ]; then
    pass "created $WANT snapshots on a $TAP_POOL_SIZE-tap pool (every snapshot succeeded)"
  else
    fail "snapshot creation failed after ${#HELD[@]} of $WANT held on a $TAP_POOL_SIZE-tap pool: $failed_at"
  fi

  req GET /snapshots >/dev/null
  listed="$(jq -r '.snapshots[].id' < "$WORKDIR/resp.json" | sort)"
  missing=0
  for snap in "${HELD[@]}"; do grep -qx "$snap" <<<"$listed" || missing=$((missing + 1)); done
  assert_eq "every snapshot taken is listed (exact ids, none lost)" "0" "$missing"

  req GET /status >/dev/null
  held_ips="$(jq -r '.ip_pool.held_by_snapshots // empty' < "$WORKDIR/resp.json")"
  if [ -n "$held_ips" ] && [ "$held_ips" -ge "${#HELD[@]}" ]; then
    pass "/status reports the snapshots' IPs as held ($held_ips >= ${#HELD[@]})"
  else
    fail "/status ip_pool.held_by_snapshots is '$held_ips', expected at least ${#HELD[@]}"
  fi
  assert_contains "/status exposes tap in-use" "$(cat "$WORKDIR/resp.json")" '"in_use"'

  status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"snapshots-hold-no-taps-create"}}')"
  assert_status "creating a sandbox while more snapshots than taps are held" 200 "$status"
  id="$(extract id < "$WORKDIR/resp.json")"
  [ -n "$id" ] && CREATED_SANDBOXES+=("$id")

  # Resume several at once: each leases its own tap and keeps its own IP.
  RESUME_COUNT=4
  for n in $(seq 1 "$RESUME_COUNT"); do
    snap="${HELD[$((n - 1))]}"
    [ -n "$snap" ] && curl -s -o "$WORKDIR/resume-$n.json" -w '%{http_code}' -X POST "$BASE_URL/snapshots/$snap/resume?retain_history=false" "${AUTH_HEADER[@]}" >"$WORKDIR/resume-$n.status" &
  done
  wait
  RESUMED=()
  for n in $(seq 1 "$RESUME_COUNT"); do
    assert_eq "concurrent resume #$n of a held snapshot" "200" "$(cat "$WORKDIR/resume-$n.status" 2>/dev/null)"
    rid="$(extract id < "$WORKDIR/resume-$n.json")"
    if [ -n "$rid" ]; then
      RESUMED+=("$rid")
      CREATED_SANDBOXES+=("$rid")
      # retain_history=false consumed the snapshot; stop tracking it.
      CREATED_SNAPSHOTS=("${CREATED_SNAPSHOTS[@]/${HELD[$((n - 1))]}}")
    fi
  done
  ips=()
  for rid in "${RESUMED[@]}"; do
    req POST "/sandboxes/$rid/exec" '{"command":"sh","args":["-c","ip -4 -o addr show eth0 | awk \"{print \\$4}\"; ping -c1 -W2 172.16.0.1 >/dev/null 2>&1 && echo gateway-ok"]}' >/dev/null
    out="$(jq -r '.stdout // empty' < "$WORKDIR/resp.json")"
    ips+=("$(head -n1 <<<"$out")")
    assert_contains "resumed sandbox ${rid:0:8} reaches the gateway on its (new) tap" "$out" "gateway-ok"
  done
  distinct="$(printf '%s\n' "${ips[@]}" | sort -u | grep -c .)"
  assert_eq "the resumed sandboxes kept distinct frozen IPs" "${#RESUMED[@]}" "$distinct"

  # Egress is keyed by tap, and the tap changes across a snapshot: the policy
  # must follow the guest. Needs one address outside the bridge that the host
  # can reach (the bridge gateway is always exempt), so it is opt-in.
  if [ -n "${SANDKILN_INTEGRATION_TEST_EGRESS_PROBE_IP:-}" ]; then
    probe="ping -c1 -W2 $SANDKILN_INTEGRATION_TEST_EGRESS_PROBE_IP >/dev/null 2>&1 && echo reachable || echo blocked"
    status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"snapshots-hold-no-taps-control"}}')"
    CTRL="$(extract id < "$WORKDIR/resp.json")"
    [ -n "$CTRL" ] && CREATED_SANDBOXES+=("$CTRL")
    req POST "/sandboxes/$CTRL/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"$probe\"]}" >/dev/null
    assert_contains "control sandbox (no policy) can reach the probe address" "$(cat "$WORKDIR/resp.json")" "reachable"

    status="$(req POST /sandboxes '{"egress":{"mode":"deny_all"},"tags":{"suite":"integration","case":"snapshots-hold-no-taps-egress"}}')"
    DENY="$(extract id < "$WORKDIR/resp.json")"
    [ -n "$DENY" ] && CREATED_SANDBOXES+=("$DENY")
    req POST "/sandboxes/$DENY/snapshot" >/dev/null
    DENY_SNAP="$(extract snapshot_id < "$WORKDIR/resp.json")"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$DENY}")
    # Occupy taps so the resume cannot land on the one it was snapshotted from.
    HOGS=()
    for _ in 1 2 3; do
      req POST /sandboxes '{"tags":{"suite":"integration","case":"snapshots-hold-no-taps-hog"}}' >/dev/null
      hog="$(extract id < "$WORKDIR/resp.json")"
      [ -n "$hog" ] && { HOGS+=("$hog"); CREATED_SANDBOXES+=("$hog"); }
    done
    status="$(req POST "/snapshots/$DENY_SNAP/resume?retain_history=false")"
    assert_status "resume the deny_all snapshot onto a different tap" 200 "$status"
    DENY2="$(extract id < "$WORKDIR/resp.json")"
    [ -n "$DENY2" ] && CREATED_SANDBOXES+=("$DENY2")
    req POST "/sandboxes/$DENY2/exec" "{\"command\":\"sh\",\"args\":[\"-c\",\"$probe\"]}" >/dev/null
    assert_contains "a deny_all sandbox is still blocked after resuming on another tap" "$(cat "$WORKDIR/resp.json")" "blocked"
  else
    echo "  skip - set SANDKILN_INTEGRATION_TEST_EGRESS_PROBE_IP to a host-reachable address outside the bridge to check egress after a tap change"
  fi

  # Fork (a resume that keeps the snapshot) leases its own tap too, and gives
  # it back: the snapshot is forkable again afterwards.
  FORK_FROM="${HELD[$RESUME_COUNT]}"
  status="$(req POST "/snapshots/$FORK_FROM/fork")"
  assert_status "fork a held snapshot (leases a tap of its own)" 200 "$status"
  FORK_ID="$(extract id < "$WORKDIR/resp.json")"
  if [ -n "$FORK_ID" ]; then
    CREATED_SANDBOXES+=("$FORK_ID")
    status="$(req DELETE "/sandboxes/$FORK_ID?keep=false")"
    assert_status "destroying the fork returns its tap" 204 "$status"
    CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$FORK_ID}")
    status="$(req POST "/snapshots/$FORK_FROM/fork")"
    assert_status "the same snapshot forks again after the first fork is gone" 200 "$status"
    FORK_ID2="$(extract id < "$WORKDIR/resp.json")"
    [ -n "$FORK_ID2" ] && CREATED_SANDBOXES+=("$FORK_ID2")
  fi

  # Free the disk this topic used before the suite moves on (the cleanup trap
  # would too, but only at exit).
  for sbx in "${CREATED_SANDBOXES[@]:-}"; do
    [ -n "$sbx" ] && req DELETE "/sandboxes/$sbx?keep=false" >/dev/null
  done
  CREATED_SANDBOXES=()
  for snap in "${HELD[@]}"; do req DELETE "/snapshots/$snap" >/dev/null; done
  CREATED_SNAPSHOTS=()
fi

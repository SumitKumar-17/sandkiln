#!/usr/bin/env bash
# End-to-end integration test against a running sandkilnd daemon: exercises
# the full API surface in one repeatable run. This file is just the
# runner — shared helpers live in scripts/lib/integration-test-helpers.sh,
# and each topic (lifecycle, drives, snapshots, fork, named sandboxes,
# images, auth, ...) is its own file under scripts/integration-tests/. Each
# topic runs as its own subshell process, in parallel with the others (see
# `run_topic` below) — a topic file's own top-level code is unchanged from
# when this all ran as one sourced shell script, but WORKDIR/PASS/FAIL/
# CREATED_SANDBOXES/etc. are now that subshell's own private copies rather
# than one shared set of globals, which is what makes running them
# concurrently safe: no two topics can stomp on each other's counters or
# `$WORKDIR/resp.json`. This is safe specifically because topic files were
# already required not to depend on another topic's leftover state (see
# "Adding a new topic" below, unchanged) — that was true before parallelism
# existed and is exactly the property that makes it safe now.
#
# This does not replace `cargo test` (pure logic, no KVM needed) or
# `scripts/load-test.sh` (concurrency/latency under load) — it's the third
# leg: does the whole real system, wired together, actually behave
# correctly end to end. Every case here was, at some point, a bug found by
# doing this exact sequence by hand (see AGENTS.md's gotchas list and
# ROADMAP.md's Benchmarking section for the history) — codifying it means
# the next feature doesn't get to reopen one of these by accident.
#
# Usage: scripts/integration-test.sh [base-url]
#   base-url defaults to $SANDKILN_INTEGRATION_TEST_URL or
#   http://127.0.0.1:7777. If SANDKILN_AUTH_TOKEN is set in the environment,
#   auth-specific checks run too; otherwise they're skipped with a note,
#   since a daemon started without SANDKILN_AUTH_TOKEN can't be told to
#   require one at request time. Likewise, if SANDKILN_JAILER_ENABLED is
#   set in this script's own environment, jailer-specific checks run too
#   (exec inside a jailed sandbox, and that snapshotting one is rejected) —
#   set it to whatever value the daemon under test was actually started
#   with jailer enabled via, so this only runs those checks when they can
#   actually pass.
#
#   SANDKILN_INTEGRATION_TEST_PARALLELISM (default 4) caps how many topic
#   files run at once. Each topic mostly runs sandboxes one at a time, but
#   they all draw from the same physical tap-device pool (32 by default,
#   see scripts/host-setup/create-tap-pool.sh) and a couple of topics
#   (18-pool.sh) hold a few sandboxes concurrently on their own — set to 1
#   to fall back to the old fully-sequential behavior, e.g. for debugging
#   a single topic's output without any interleaving.
#
#   Measured: ~4m10s sequential (parallelism=1) down to ~70-90s at the
#   default of 4 on the dev box, a real ~2.8-3.5x cut. 18-pool.sh's own
#   timing-sensitive assertions (a real background replenish cycle
#   racing a bounded wait, not a mocked timer) are the one thing that's
#   measurably more flake-prone under concurrency than the rest of this
#   suite — three full runs at parallelism=4 landed 300/300, 299/300, and
#   295/300, with every failure isolated to that one topic and none of
#   them reproducing on a re-run. Bumped its wait windows once (30s/8
#   tries -> 60s/15 tries) rather than chasing this further; a topic
#   whose own doc comments already call out "this dev box's own
#   concurrent load" as a factor was already living with this class of
#   flakiness before parallelism existed, just less often.
#
# Every sandbox/drive/snapshot this script creates is tracked and torn
# down on exit, pass or fail — see the cleanup trap below.
#
# Adding a new topic: drop a new numbered file in scripts/integration-tests/
# (the number just controls display order — nothing here depends on one
# topic's leftover state from another, each section creates and cleans up
# its own resources) and it runs automatically; no change needed here.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BASE_URL="${1:-${SANDKILN_INTEGRATION_TEST_URL:-http://127.0.0.1:7777}}"
BASE_URL="${BASE_URL%/}"
AUTH_TOKEN="${SANDKILN_AUTH_TOKEN:-}"
PARALLELISM="${SANDKILN_INTEGRATION_TEST_PARALLELISM:-4}"

if ! curl -sf -o /dev/null "$BASE_URL/healthz"; then
  echo "sandkilnd not reachable at $BASE_URL/healthz — is it running?" >&2
  exit 1
fi
echo "sandkiln integration test against $BASE_URL (parallelism: $PARALLELISM)"
[ -z "$AUTH_TOKEN" ] && echo "(SANDKILN_AUTH_TOKEN not set — auth checks will be skipped)"

# A few seconds before "now" absorbs clock skew between this shell and the
# daemon's clock in the final history sweep below, without risking
# sweeping up something genuinely pre-existing.
SUITE_STARTED_UNIX="$(($(date +%s) - 5))"
LOGDIR="$(mktemp -d /tmp/sandkiln-integration-logs-XXXXXX)"
trap 'rm -rf "$LOGDIR"' EXIT

# Runs one topic file to completion in its own subshell: its own WORKDIR,
# its own PASS/FAIL/FAILURES/CREATED_* — a fresh, private copy of exactly
# what every topic file already assumes exists in its top-level scope, so
# no topic file itself needed to change for this to be parallel-safe.
# Output (both the topic's own `pass`/`fail` lines and this wrapper's own
# result line) goes to $LOGDIR/<name>.log rather than straight to stdout,
# since several of these run at once and interleaved output line-by-line
# would be unreadable; the parent prints each log in full, in order, once
# every topic has finished (see the aggregation loop below).
#
# Deliberately does NOT do the untracked-checkpoint history sweep that
# used to live in this function when it was the single top-level
# `cleanup()` -- with several topics resuming/forking snapshots at once,
# each one sweeping "anything retired since I started" would race: topic
# A finishing and sweeping could delete a checkpoint topic B (still
# running, started around the same time) still needs. That sweep now runs
# exactly once, after every topic has finished, at the bottom of this
# file.
run_topic() {
  local topic="$1" name
  name="$(basename "$topic" .sh)"
  (
    WORKDIR="$(mktemp -d /tmp/sandkiln-integration-test-XXXXXX)"
    PASS=0
    FAIL=0
    FAILURES=()
    CREATED_SANDBOXES=()
    CREATED_DRIVES=()
    CREATED_SNAPSHOTS=()
    CREATED_IMAGES=()
    CREATED_POOLS=()
    AUTH_HEADER=()
    [ -n "$AUTH_TOKEN" ] && AUTH_HEADER=(-H "Authorization: Bearer $AUTH_TOKEN")

    cleanup() {
      # ?keep=false: cleanup means "get rid of everything this topic
      # created", not "leave a snapshot behind" — DELETE now preserves
      # state by default (see integration-tests/12-named-sandboxes.sh),
      # which would otherwise leak an untracked snapshot on every run.
      for id in "${CREATED_SANDBOXES[@]:-}"; do
        [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/sandboxes/$id?keep=false" "${AUTH_HEADER[@]}"
      done
      for id in "${CREATED_SNAPSHOTS[@]:-}"; do
        [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/snapshots/$id" "${AUTH_HEADER[@]}"
      done
      for id in "${CREATED_DRIVES[@]:-}"; do
        [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/drives/$id" "${AUTH_HEADER[@]}"
      done
      for id in "${CREATED_IMAGES[@]:-}"; do
        [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/images/$id" "${AUTH_HEADER[@]}"
      done
      for id in "${CREATED_POOLS[@]:-}"; do
        [ -n "$id" ] && curl -s -o /dev/null -X DELETE "$BASE_URL/pools/$id" "${AUTH_HEADER[@]}"
      done
      rm -rf "$WORKDIR"
    }
    trap cleanup EXIT

    # shellcheck source=lib/integration-test-helpers.sh
    source "$SCRIPT_DIR/lib/integration-test-helpers.sh"
    # shellcheck disable=SC1090
    source "$topic"

    echo "___SANDKILN_TOPIC_RESULT___ PASS=$PASS FAIL=$FAIL"
    for f in "${FAILURES[@]:-}"; do
      [ -n "$f" ] && echo "___SANDKILN_TOPIC_FAILURE___ $name: $f"
    done
  ) >"$LOGDIR/$name.log" 2>&1
}

# Bounded parallelism via bash job control: launch topics up to
# $PARALLELISM at a time, waiting for at least one to finish (`wait -n`)
# before starting the next once the cap is hit.
running=0
for topic in "$SCRIPT_DIR"/integration-tests/*.sh; do
  run_topic "$topic" &
  running=$((running + 1))
  if [ "$running" -ge "$PARALLELISM" ]; then
    wait -n
    running=$((running - 1))
  fi
done
wait

TOTAL_PASS=0
TOTAL_FAIL=0
ALL_FAILURES=()
for log in "$LOGDIR"/*.log; do
  cat "$log"
  result_line="$(grep '^___SANDKILN_TOPIC_RESULT___' "$log" || true)"
  topic_pass="$(echo "$result_line" | sed -E 's/.*PASS=([0-9]+).*/\1/')"
  topic_fail="$(echo "$result_line" | sed -E 's/.*FAIL=([0-9]+).*/\1/')"
  TOTAL_PASS=$((TOTAL_PASS + ${topic_pass:-0}))
  TOTAL_FAIL=$((TOTAL_FAIL + ${topic_fail:-0}))
  while IFS= read -r f; do
    ALL_FAILURES+=("${f#___SANDKILN_TOPIC_FAILURE___ }")
  done < <(grep '^___SANDKILN_TOPIC_FAILURE___' "$log" || true)
done

# The one deferred cleanup step every per-topic subshell above
# deliberately skipped -- see run_topic's own comment on why this has to
# happen exactly once, after every topic has already finished, not once
# per topic while others might still be running.
AUTH_HEADER=()
[ -n "$AUTH_TOKEN" ] && AUTH_HEADER=(-H "Authorization: Bearer $AUTH_TOKEN")
if command -v jq >/dev/null 2>&1; then
  for id in $(curl -s "$BASE_URL/snapshots/history" "${AUTH_HEADER[@]}" | jq -r --argjson cutoff "$SUITE_STARTED_UNIX" '.checkpoints[] | select(.retired_at_unix >= $cutoff) | .id' 2>/dev/null); do
    curl -s -o /dev/null -X DELETE "$BASE_URL/snapshots/history/$id" "${AUTH_HEADER[@]}"
  done
fi

echo
echo "=== results ==="
echo "passed: $TOTAL_PASS   failed: $TOTAL_FAIL"
if [ "$TOTAL_FAIL" -gt 0 ]; then
  echo
  echo "failures:"
  for f in "${ALL_FAILURES[@]}"; do
    echo "  - $f"
  done
fi

[ "$TOTAL_FAIL" -eq 0 ]

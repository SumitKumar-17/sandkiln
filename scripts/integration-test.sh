#!/usr/bin/env bash
# End-to-end integration test against a running sandkilnd daemon: exercises
# the full API surface in one repeatable run. This file is just the
# runner — shared helpers live in scripts/lib/integration-test-helpers.sh,
# and each topic (lifecycle, drives, snapshots, fork, named sandboxes,
# images, auth, ...) is its own file under scripts/integration-tests/,
# sourced in order below. All of it shares one shell process (`source`,
# not a subshell per file), so globals like WORKDIR/PASS/FAIL/
# CREATED_SANDBOXES and every assert_*/req helper are visible in every
# topic file exactly as if this were still one long script — splitting
# it up is purely organizational, not a change in how it runs.
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
# Every sandbox/drive/snapshot this script creates is tracked and torn
# down on exit, pass or fail — see the cleanup trap below.
#
# Adding a new topic: drop a new numbered file in scripts/integration-tests/
# (the number just controls run order — nothing here depends on one topic's
# leftover state from another, each section creates and cleans up its own
# resources) and it runs automatically; no change needed here.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

BASE_URL="${1:-${SANDKILN_INTEGRATION_TEST_URL:-http://127.0.0.1:7777}}"
BASE_URL="${BASE_URL%/}"
AUTH_TOKEN="${SANDKILN_AUTH_TOKEN:-}"

WORKDIR="$(mktemp -d /tmp/sandkiln-integration-test-XXXXXX)"
PASS=0
FAIL=0
FAILURES=()

CREATED_SANDBOXES=()
CREATED_DRIVES=()
CREATED_SNAPSHOTS=()
CREATED_IMAGES=()
CREATED_POOLS=()

cleanup() {
  # ?keep=false: cleanup means "get rid of everything this run created",
  # not "leave a snapshot behind" — DELETE now preserves state by
  # default (see integration-tests/12-named-sandboxes.sh), which would
  # otherwise leak an untracked snapshot on every run of this script.
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

AUTH_HEADER=()
[ -n "$AUTH_TOKEN" ] && AUTH_HEADER=(-H "Authorization: Bearer $AUTH_TOKEN")

# shellcheck source=lib/integration-test-helpers.sh
source "$SCRIPT_DIR/lib/integration-test-helpers.sh"

# ---------------------------------------------------------------------------
if ! curl -sf -o /dev/null "$BASE_URL/healthz"; then
  echo "sandkilnd not reachable at $BASE_URL/healthz — is it running?" >&2
  exit 1
fi
echo "sandkiln integration test against $BASE_URL"
[ -z "$AUTH_TOKEN" ] && echo "(SANDKILN_AUTH_TOKEN not set — auth checks will be skipped)"

for topic in "$SCRIPT_DIR"/integration-tests/*.sh; do
  # shellcheck disable=SC1090
  source "$topic"
done

section "results"
echo "passed: $PASS   failed: $FAIL"
if [ "$FAIL" -gt 0 ]; then
  echo
  echo "failures:"
  for f in "${FAILURES[@]}"; do
    echo "  - $f"
  done
fi

[ "$FAIL" -eq 0 ]

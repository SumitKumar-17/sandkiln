#!/usr/bin/env bash
# Shared state and helper functions for the integration test suite —
# sourced (not executed) by scripts/integration-test.sh before it sources
# each topic file in scripts/integration-tests/, so every function and
# global here (WORKDIR, PASS/FAIL/FAILURES, req/assert_*, etc.) is
# available in every topic file's own top-level scope, as if it had all
# been one file. Nothing in here is meant to run standalone.

extract() {
  # extract <json-field> — reads stdin, prints that top-level string field.
  local field="$1"
  if command -v jq >/dev/null 2>&1; then
    jq -r ".${field} // empty"
  else
    grep -o "\"${field}\"[[:space:]]*:[[:space:]]*\"[^\"]*\"" | sed -E 's/.*:"([^"]*)"/\1/'
  fi
}

req() {
  # req <method> <path> [json-body] — writes the response body to
  # $WORKDIR/resp.json, prints the HTTP status code.
  local method="$1" path="$2" data="${3:-}"
  if [ -n "$data" ]; then
    curl -s -o "$WORKDIR/resp.json" -w '%{http_code}' -X "$method" "$BASE_URL$path" \
      -H 'Content-Type: application/json' "${AUTH_HEADER[@]}" -d "$data"
  else
    curl -s -o "$WORKDIR/resp.json" -w '%{http_code}' -X "$method" "$BASE_URL$path" "${AUTH_HEADER[@]}"
  fi
}

pass() {
  PASS=$((PASS + 1))
  echo "  ok   - $1"
}

fail() {
  FAIL=$((FAIL + 1))
  FAILURES+=("$1")
  echo "  FAIL - $1"
}

assert_status() {
  # assert_status <description> <expected> <actual>
  if [ "$3" = "$2" ]; then
    pass "$1"
  else
    fail "$1 (expected http $2, got $3; body: $(cat "$WORKDIR/resp.json" 2>/dev/null | head -c 200))"
  fi
}

assert_eq() {
  # assert_eq <description> <expected> <actual>
  if [ "$2" = "$3" ]; then
    pass "$1"
  else
    fail "$1 (expected '$2', got '$3')"
  fi
}

assert_contains() {
  # assert_contains <description> <haystack> <needle>
  if [[ "$2" == *"$3"* ]]; then
    pass "$1"
  else
    fail "$1 (expected to find '$3')"
  fi
}

assert_not_contains() {
  # assert_not_contains <description> <haystack> <needle>
  if [[ "$2" != *"$3"* ]]; then
    pass "$1"
  else
    fail "$1 (expected NOT to find '$3')"
  fi
}

section() { echo; echo "=== $1 ==="; }

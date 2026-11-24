#!/usr/bin/env bash
# Assertions over the stub daemon's request log ($STUB_LOG, JSON lines).
#
# Usage: assert-requests.sh destroyed <sandbox-id>   DELETE ?keep=false seen, exactly once
#        assert-requests.sh streamed <sandbox-id>    exec-stream started and its log socket attached
#        assert-requests.sh authenticated <id>       every API request carried the right token
#        assert-requests.sh refused                  a request was rejected for a wrong token and created nothing
#        assert-requests.sh none                     no sandbox was created or touched
# Prints the matching log lines, exits 1 when the assertion fails.

set -euo pipefail

: "${STUB_LOG:?STUB_LOG is not set (start-stub.sh sets it)}"
cmd=${1:-}
id=${2:-}

# Count log lines for which a jq predicate holds, ignoring the readiness probe.
count() { jq -s --arg id "$id" "[.[] | select(.path != \"/healthz\") | select($1)] | length" "$STUB_LOG"; }
expect() { # description, jq predicate, wanted count
  local got
  got=$(count "$2")
  if [ "$got" = "$3" ]; then echo "ok   $1 ($got)"; else echo "FAIL $1: got $got, want $3"; echo "--- request log"; jq -c '{method, path, status, auth, keep}' "$STUB_LOG"; exit 1; fi
}

case "$cmd" in
  destroyed)
    [ -n "$id" ] || { echo "FAIL no sandbox id was reported"; exit 1; }
    expect "DELETE ?keep=false for $id" '.method == "DELETE" and .sandbox == $id and .keep == "false" and .status == 204' 1 ;;
  streamed)
    expect "exec-stream started" '.method == "POST" and .sandbox == $id and (.path | endswith("/exec-stream")) and .status == 200' 1
    expect "log socket attached" '.method == "WS" and .sandbox == $id and .status == 101' 1
    expect "workspace archive uploaded" '.method == "POST" and .sandbox == $id and .body.path == "/tmp/sandkiln-workspace.tgz" and .status == 204' 1 ;;
  authenticated)
    expect "requests for $id" '.sandbox == $id' "$(count '.sandbox == $id')"
    expect "no request without the right token" '.sandbox == $id and .auth != "ok"' 0 ;;
  refused)
    expect "a wrong token was refused" '.auth == "bad" and .status == 401' 1
    expect "exactly one sandbox was created" '.method == "POST" and .path == "/sandboxes" and .status == 200' 1 ;;
  none)
    expect "nothing but the readiness probe reached the stub" 'true' 0 ;;
  *) echo "usage: assert-requests.sh destroyed|streamed|authenticated|refused|none [sandbox-id]" >&2; exit 2 ;;
esac

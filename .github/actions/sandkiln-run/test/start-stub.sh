#!/usr/bin/env bash
# Start the stub daemon in the background and wait until it answers.
#
# Usage: start-stub.sh [--token-env VAR]
#   --token-env VAR   require the bearer token held in environment variable VAR
#
# Writes STUB_URL and STUB_LOG to $GITHUB_ENV when set (else prints them), so
# later steps can point the action at the stub and read the recorded requests.
# Needs Node.js 18+ and network access to the npm registry (for `ws`).

set -euo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
PORT=${STUB_PORT:-7777}
token_env=""
while [ $# -gt 0 ]; do
  case "$1" in
    --token-env) token_env=${2:?--token-env needs a variable name}; shift 2 ;;
    -h|--help) sed -n 2,10p "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

command -v node >/dev/null || { echo "node is required" >&2; exit 2; }
work=${RUNNER_TEMP:-$(mktemp -d)}
log="$work/stub-requests.jsonl"
: >"$log"

(cd "$HERE" && npm ci --no-audit --no-fund --silent)

env STUB_PORT="$PORT" STUB_LOG="$log" ${token_env:+STUB_TOKEN="${!token_env}"} \
  nohup node "$HERE/stub-daemon.mjs" >"$work/stub.out" 2>&1 &

for _ in $(seq 1 50); do
  if curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
    if [ -n "${GITHUB_ENV:-}" ]; then
      { echo "STUB_URL=http://127.0.0.1:$PORT"; echo "STUB_LOG=$log"; } >>"$GITHUB_ENV"
    else
      echo "STUB_URL=http://127.0.0.1:$PORT"; echo "STUB_LOG=$log"
    fi
    exit 0
  fi
  sleep 0.2
done
echo "stub daemon did not become ready:" >&2
cat "$work/stub.out" >&2
exit 1

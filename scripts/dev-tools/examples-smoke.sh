#!/usr/bin/env bash
# Runs every example under examples/ the way its README tells a new user to,
# against a live daemon, and prints a pass/fail/skip table. Each example runs
# from a throwaway copy, so nothing is installed into the repository, and every
# sandbox, snapshot, pool and named workspace the examples create is deleted
# afterwards (matched by the `example` tag or the example's own names, never by
# a global sweep, so other users of the daemon are untouched).
#
# Usage: scripts/dev-tools/examples-smoke.sh [options]
#   --base-url URL     daemon to test (default $SANDKILN_DAEMON_URL, else
#                      http://127.0.0.1:7777). SANDKILN_AUTH_TOKEN is honoured.
#   --published        install the `sandkiln` packages from the registry, the way
#                      a user would. Default: link this repository's build
#                      (packages/sdk/dist, packages/python, packages/cli/dist),
#                      so unreleased SDK code is what gets exercised.
#   --only a,b         run only these examples
#   --skip a,b         skip these examples
#   --keep-work        leave the temporary working copies in place (debugging)
#   -h, --help         this text
#
# Needs: node >= 22.18, curl. Optional (the example is SKIPPED, with the reason,
# when missing): python3 + venv for agent-runner, `script` for
# interactive-terminal, a default route or EGRESS_EXAMPLE_TARGET_IP for
# egress-policy, S3_ENDPOINT/S3_ACCESS_KEY/S3_SECRET_KEY/S3_BUCKET (and a
# FUSE+rclone daemon, see SELF_HOSTING.md) for remote-storage-mount.
# Run from the daemon host or point --base-url at it; the local-tunnel example
# serves from this machine, so the daemon must be able to reach it (it opens the
# tunnel from this side, so any base URL works).
#
# Exit status: 0 if nothing failed (skips do not fail), 1 otherwise.

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EXAMPLES="$REPO_ROOT/examples"

BASE_URL="${SANDKILN_DAEMON_URL:-http://127.0.0.1:7777}"
PUBLISHED=0
KEEP_WORK=0
ONLY=""
SKIP=""

usage() { awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "${BASH_SOURCE[0]}"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --base-url) BASE_URL="${2:?--base-url needs a value}"; shift 2 ;;
    --published) PUBLISHED=1; shift ;;
    --only) ONLY=",${2:?--only needs a value},"; shift 2 ;;
    --skip) SKIP=",${2:?--skip needs a value},"; shift 2 ;;
    --keep-work) KEEP_WORK=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done
BASE_URL="${BASE_URL%/}"
export SANDKILN_DAEMON_URL="$BASE_URL"

# ---- prerequisites -----------------------------------------------------------

die() { echo "examples-smoke: $*" >&2; exit 2; }

command -v node >/dev/null || die "node not found (need >= 22.18)"
command -v curl >/dev/null || die "curl not found"
node -e 'const [a,b]=process.versions.node.split(".").map(Number); process.exit(a>22||(a===22&&b>=18)?0:1)' \
  || die "node $(node -v) is too old: need >= 22.18 (global WebSocket, TypeScript type stripping)"

AUTH=()
[ -n "${SANDKILN_AUTH_TOKEN:-}" ] && AUTH=(-H "Authorization: Bearer $SANDKILN_AUTH_TOKEN")
curl -sf -o /dev/null "${AUTH[@]}" "$BASE_URL/healthz" || die "no daemon reachable at $BASE_URL/healthz"

if [ "$PUBLISHED" = 0 ]; then
  [ -f "$REPO_ROOT/packages/sdk/dist/index.js" ] \
    || die "packages/sdk/dist is missing: run 'npm ci && npm run build' at the repository root, or pass --published"
else
  command -v npm >/dev/null || die "npm not found (needed for --published)"
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/examples-smoke.XXXXXX")"
BG_PIDS=()

# ---- daemon cleanup ----------------------------------------------------------

# Names this run owns. `example` tag values come from each example's own
# `tags: { example: ... }`; the named workspace and the pool id prefix are
# the examples' fixed names. A pre-existing named workspace is never touched
# (see example_named_persistent_sandbox).
OWNED_NAMED_WORKSPACE=0

cleanup_daemon() {
  OWNED_NAMED_WORKSPACE="$OWNED_NAMED_WORKSPACE" node --input-type=module - <<'JS'
const base = process.env.SANDKILN_DAEMON_URL;
const token = process.env.SANDKILN_AUTH_TOKEN;
const headers = token ? { authorization: `Bearer ${token}` } : {};
const tagged = (t) => t && typeof t.example === "string";
const named = process.env.OWNED_NAMED_WORKSPACE === "1" ? "example-counter-agent" : null;
const get = async (p) => (await fetch(base + p, { headers })).json();
const del = async (p) => { await fetch(base + p, { method: "DELETE", headers }).catch(() => {}); };
let removed = 0;
for (const s of (await get("/sandboxes")).sandboxes ?? []) {
  if (tagged(s.tags) || (named && s.name === named)) { await del(`/sandboxes/${s.id}?keep=false`); removed++; }
}
for (const s of (await get("/snapshots")).snapshots ?? []) {
  if (tagged(s.tags) || (named && s.name === named)) { await del(`/snapshots/${s.id}`); removed++; }
}
for (const p of (await get("/pools")).pools ?? []) {
  if (p.id.startsWith("pool-warm-start-example-")) { await del(`/pools/${encodeURIComponent(p.id)}`); removed++; }
}
if (removed > 0) console.log(`cleanup: removed ${removed} leftover example resource(s)`);
JS
}

finish() {
  local pid
  for pid in "${BG_PIDS[@]:-}"; do [ -n "$pid" ] && kill "$pid" 2>/dev/null; done
  cleanup_daemon
  if [ "$KEEP_WORK" = 1 ]; then echo "working copies kept in $WORK"; else rm -rf "$WORK"; fi
}
trap finish EXIT
trap 'exit 130' INT TERM

# ---- result bookkeeping ------------------------------------------------------

NAMES=(); STATUS=(); NOTES=(); SECS=()
SKIP_REASON=""
FAIL_NOTE=""

selected() {
  [ -z "$ONLY" ] || [[ "$ONLY" == *",$1,"* ]] || return 1
  [[ "$SKIP" != *",$1,"* ]]
}

record() { NAMES+=("$1"); STATUS+=("$2"); NOTES+=("$3"); SECS+=("$4"); }

# Run one example function. It returns 0 (pass), 1 (fail; FAIL_NOTE says why),
# or 3 (skip; SKIP_REASON says why).
run_example() {
  local name="$1" fn="example_${1//-/_}" start rc
  selected "$name" || return 0
  echo "==> $name"
  SKIP_REASON=""; FAIL_NOTE=""
  start=$SECONDS
  "$fn"; rc=$?
  case $rc in
    0) record "$name" PASS "" $((SECONDS - start)) ;;
    3) record "$name" SKIP "$SKIP_REASON" 0 ;;
    *) record "$name" FAIL "$FAIL_NOTE (log: $WORK/$name.out)" $((SECONDS - start))
       [ -f "$WORK/$name.out" ] && tail -n 15 "$WORK/$name.out" | sed 's/^/    | /' ;;
  esac
}

fail() { FAIL_NOTE="$1"; return 1; }
skip() { SKIP_REASON="$1"; return 3; }
expect() { grep -Eq -- "$2" "$WORK/$1.out" || { FAIL_NOTE="output lacks /$2/"; return 1; }; }

# Copy an example into the work dir and make `sandkiln` resolve: a symlink to
# this repository's build, or a real registry install with --published.
prepare_js() {
  local name="$1"
  mkdir -p "$WORK/$name"
  (cd "$EXAMPLES/$name" && tar --exclude=node_modules --exclude=__pycache__ -cf - .) | tar -xf - -C "$WORK/$name"
  if [ "$PUBLISHED" = 1 ]; then
    (cd "$WORK/$name" && npm install --silent --no-audit --no-fund >"$WORK/$name.install" 2>&1) \
      || { FAIL_NOTE="npm install from the registry failed (see $WORK/$name.install)"; return 1; }
  else
    mkdir -p "$WORK/$name/node_modules"
    ln -s "$REPO_ROOT/packages/sdk" "$WORK/$name/node_modules/sandkiln"
  fi
}

# run_js <example> <timeout-secs> <node args...>; output lands in $WORK/<example>.out.
run_js() {
  local name="$1" secs="$2"; shift 2
  (cd "$WORK/$name" && timeout "$secs" node "$@") >"$WORK/$name.out" 2>&1
}

# Simple examples: prepare, run, exit 0, and every pattern present.
simple_js() {
  local name="$1" secs="$2" script="$3"; shift 3
  prepare_js "$name" || return 1
  run_js "$name" "$secs" "$script" || { FAIL_NOTE="exit code $?"; return 1; }
  local pattern
  for pattern in "$@"; do expect "$name" "$pattern" || return 1; done
}

# ---- examples ----------------------------------------------------------------

example_code_playground() {
  prepare_js code-playground || return 1
  run_js code-playground 120 index.js hello.py || { FAIL_NOTE="exit code $? (the image needs python3)"; return 1; }
  expect code-playground "hello from inside a sandkiln microVM"
}

example_agent_runner() {
  command -v python3 >/dev/null || { skip "python3 not found"; return; }
  local dir="$WORK/agent-runner"
  mkdir -p "$dir"
  cp "$EXAMPLES/agent-runner/main.py" "$EXAMPLES/agent-runner/requirements.txt" "$dir/"
  python3 -m venv "$dir/.venv" >"$WORK/agent-runner.install" 2>&1 || { skip "python3 -m venv unavailable (install python3-venv)"; return; }
  if [ "$PUBLISHED" = 1 ]; then
    "$dir/.venv/bin/pip" install -q -r "$dir/requirements.txt" >>"$WORK/agent-runner.install" 2>&1 \
      || { FAIL_NOTE="pip install from the registry failed"; return 1; }
  else
    "$dir/.venv/bin/pip" install -q "$REPO_ROOT/packages/python" >>"$WORK/agent-runner.install" 2>&1 \
      || { FAIL_NOTE="pip install of packages/python failed"; return 1; }
  fi
  (cd "$dir" && timeout 120 .venv/bin/python main.py) >"$WORK/agent-runner.out" 2>&1 || { FAIL_NOTE="exit code $? (the image needs python3)"; return 1; }
  expect agent-runner '"answer": 4'
}

example_dev_server_preview() {
  prepare_js dev-server-preview || return 1
  local out="$WORK/dev-server-preview.out" pid url i
  (cd "$WORK/dev-server-preview" && exec node index.js) >"$out" 2>&1 &
  pid=$!; BG_PIDS+=("$pid")
  for i in $(seq 1 60); do grep -q "Preview it at" "$out" && break; kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
  url="$(grep -Eo 'https?://[^ ]+' "$out" | head -n 1)"
  [ -n "$url" ] || { kill "$pid" 2>/dev/null; wait "$pid" 2>/dev/null; { fail "no preview URL printed"; return 1; }; }
  curl -fsS --max-time 15 "$url" >"$WORK/dev-server-preview.body" 2>>"$out" \
    || { kill -INT "$pid"; wait "$pid" 2>/dev/null; { fail "preview URL did not answer"; return 1; }; }
  # What Ctrl+C does: the example must stop its sandbox and exit by itself.
  kill -INT "$pid"
  for i in $(seq 1 40); do kill -0 "$pid" 2>/dev/null || break; sleep 0.5; done
  if kill -0 "$pid" 2>/dev/null; then kill "$pid"; { fail "still running 20s after SIGINT"; return 1; }; fi
  wait "$pid" 2>/dev/null
  grep -q "hello from inside a sandkiln microVM" "$WORK/dev-server-preview.body" || { fail "preview body was not the sandbox's server"; return 1; }
  expect dev-server-preview "Sandbox .* stopped"
}

example_interactive_terminal() {
  command -v script >/dev/null || { skip "'script' (util-linux) not found, so no pseudo-terminal to drive"; return; }
  prepare_js interactive-terminal || return 1
  # Type into a real pty: raw mode, an echoed command, a tty check and exit.
  (sleep 6; printf 'echo pty-ok-$((6*7)); tty; exit\r'; sleep 4) \
    | (cd "$WORK/interactive-terminal" && timeout 90 script -qec "node index.js" /dev/null) >"$WORK/interactive-terminal.out" 2>&1
  expect interactive-terminal "pty-ok-42" && expect interactive-terminal "/dev/pts/" && expect interactive-terminal "Sandbox .* stopped"
}

example_snapshot_lifecycle() {
  simple_js snapshot-lifecycle 180 index.js "branch A's write did not leak into branch B" "the snapshot is gone" "^Done\\."
}

example_named_persistent_sandbox() {
  # The example's name is fixed; refuse to touch a workspace someone else owns.
  if curl -sf "${AUTH[@]}" "$BASE_URL/sandboxes" "$BASE_URL/snapshots" | grep -q '"name":"example-counter-agent"'; then
    skip 'a sandbox or snapshot named "example-counter-agent" already exists; not touching it'; return
  fi
  OWNED_NAMED_WORKSPACE=1
  simple_js named-persistent-sandbox 240 index.js "run count: 0 -> 1" "run count: 1 -> 2" "^Done\\."
}

example_pool_warm_start() {
  simple_js pool-warm-start 300 index.js "Attempt 5/5" "deleted\\."
}

example_exec_stream_logs() {
  simple_js exec-stream-logs 120 index.js "build finished" "Reattaching to the same, now-finished session" "Sandbox .* stopped"
}

example_local_tunnel() {
  simple_js local-tunnel 120 index.js "^Match -- the sandbox really did reach"
}

example_egress_policy() {
  local target="${EGRESS_EXAMPLE_TARGET_IP:-}"
  if [ -z "$target" ]; then
    case "$BASE_URL" in
      http://127.0.0.1* | http://localhost*) target="$(ip route show default 2>/dev/null | awk '/default/ {print $3; exit}')" ;;
    esac
  fi
  [ -n "$target" ] || { skip "set EGRESS_EXAMPLE_TARGET_IP to an address the daemon host can reach"; return; }
  prepare_js egress-policy || return 1
  (cd "$WORK/egress-policy" && EGRESS_EXAMPLE_TARGET_IP="$target" timeout 180 node index.js) >"$WORK/egress-policy.out" 2>&1 \
    || { FAIL_NOTE="exit code $? against $target"; return 1; }
  expect egress-policy "deny_all blocked an unlisted destination" && expect egress-policy "allow_cidrs opened it back up"
}

example_remote_storage_mount() {
  local v
  for v in S3_ENDPOINT S3_ACCESS_KEY S3_SECRET_KEY S3_BUCKET; do
    [ -n "${!v:-}" ] || { skip "needs a FUSE+rclone daemon and an S3 endpoint ($v unset)"; return; }
  done
  prepare_js remote-storage-mount || return 1
  run_js remote-storage-mount 180 index.js || { FAIL_NOTE="exit code $?"; return 1; }
  expect remote-storage-mount "Read it back byte-for-byte" && expect remote-storage-mount "no longer a mount point"
}

example_benchmark_suite() {
  prepare_js benchmark-suite || return 1
  (cd "$WORK/benchmark-suite" && timeout 120 node --test test/*.test.ts) >"$WORK/benchmark-suite.out" 2>&1 \
    || { FAIL_NOTE="unit tests failed"; return 1; }
  run_js benchmark-suite 180 src/cli.ts --iterations 1 --exec-samples 5 --only create,exec --allow-busy \
    || { FAIL_NOTE="exit code $?"; return 1; }
  expect benchmark-suite "exec_roundtrip" && expect benchmark-suite "create_to_first_command"
}

example_github_actions() {
  local dir="$WORK/github-actions" kiln_bin
  mkdir -p "$dir/bin"
  if [ "$PUBLISHED" = 1 ]; then
    command -v kiln >/dev/null || { skip "kiln is not on PATH (npm install -g sandkiln-cli)"; return; }
  else
    [ -f "$REPO_ROOT/packages/cli/dist/index.js" ] || { skip "packages/cli/dist is missing (npm run build)"; return; }
    kiln_bin="$dir/bin/kiln"
    printf '#!/bin/sh\nexec node "%s/packages/cli/dist/index.js" "$@"\n' "$REPO_ROOT" >"$kiln_bin"
    chmod +x "$kiln_bin"
  fi
  local out="$WORK/github-actions.out" gh_out="$dir/github-output" rc
  : >"$gh_out"
  local run=(env "PATH=$dir/bin:$PATH" "INPUT_DAEMON-URL=$BASE_URL" "INPUT_AUTH-TOKEN=${SANDKILN_AUTH_TOKEN:-}"
    INPUT_EGRESS=deny_all INPUT_WORKING-DIRECTORY=sample-project "GITHUB_WORKSPACE=$EXAMPLES/github-actions"
    "RUNNER_TEMP=$dir" "GITHUB_OUTPUT=$gh_out" "INPUT_COMMAND=python3 -m unittest discover -v tests"
    "$REPO_ROOT/.github/actions/sandkiln-run/run.sh")
  "${run[@]}" run >"$out" 2>&1; rc=$?
  "${run[@]}" cleanup >>"$out" 2>&1
  [ "$rc" = 0 ] || { FAIL_NOTE="run.sh exit code $rc (the image needs python3)"; return 1; }
  grep -q '^exit-code=0$' "$gh_out" || { fail "GITHUB_OUTPUT lacks exit-code=0"; return 1; }
  expect github-actions "Ran 5 tests" && expect github-actions "stopped and destroyed"
}

# ---- run ---------------------------------------------------------------------

echo "examples-smoke: $BASE_URL, sandkiln from $([ "$PUBLISHED" = 1 ] && echo "the registry" || echo "this repository's build")"
echo

run_example code-playground
run_example agent-runner
run_example dev-server-preview
run_example interactive-terminal
run_example snapshot-lifecycle
run_example named-persistent-sandbox
run_example pool-warm-start
run_example exec-stream-logs
run_example local-tunnel
run_example egress-policy
run_example remote-storage-mount
run_example benchmark-suite
run_example github-actions

echo
printf '%-26s %-5s %5s  %s\n' EXAMPLE RESULT SECS NOTE
failed=0; passed=0; skipped=0
for i in "${!NAMES[@]}"; do
  printf '%-26s %-5s %5s  %s\n' "${NAMES[$i]}" "${STATUS[$i]}" "${SECS[$i]}" "${NOTES[$i]}"
  case "${STATUS[$i]}" in PASS) passed=$((passed + 1)) ;; SKIP) skipped=$((skipped + 1)) ;; *) failed=$((failed + 1)) ;; esac
done
echo
echo "$passed passed, $failed failed, $skipped skipped"
[ "$failed" = 0 ]

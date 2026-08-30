#!/usr/bin/env bash
# One-command daemon lifecycle control: build, preflight-check, grant
# CAP_NET_ADMIN if missing, start sandkilnd in the background, wait for it
# to actually answer /healthz, and track it by PID so stop/restart/status
# are real operations instead of "go find the process by hand." This is
# the direct-run counterpart to scripts/install-systemd-service.sh — reach
# for that one for a persistent, supervised, boot-surviving deployment;
# reach for this one for a normal edit/rebuild/restart dev loop where
# installing a systemd unit is more ceremony than the moment calls for.
#
# Usage:
#   scripts/sandkilnd-ctl.sh start [--no-build] [--no-preflight]
#   scripts/sandkilnd-ctl.sh stop
#   scripts/sandkilnd-ctl.sh restart [--no-build] [--no-preflight]
#   scripts/sandkilnd-ctl.sh status
#   scripts/sandkilnd-ctl.sh logs [-f]
#
# Every SANDKILN_* env var the daemon itself reads (see
# core/crates/daemon/src/config.rs and SELF_HOSTING.md's configuration
# table) is passed straight through — set them the same way you would to
# run sandkilnd directly. This script adds exactly two of its own:
#   SANDKILN_CTL_PID_FILE   default: /tmp/sandkiln-daemon.pid
#   SANDKILN_CTL_LOG_FILE   default: /tmp/sandkiln-daemon.log
#
# Safe to run repeatedly: `start` on an already-running daemon is a no-op
# (prints its PID and exits 0), `stop` on a non-running one is a no-op.

set -uo pipefail

# rustup's cargo isn't on PATH in a non-interactive/non-login shell (it's
# added via a line sourced from .bashrc/.profile) — this bit this project
# more than once when driving builds over `ssh host 'cmd'` rather than an
# interactive session. Source it here if present so `start`'s build step
# doesn't silently fail with "cargo: command not found" in exactly that case.
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CORE_DIR="$REPO_ROOT/core"
DAEMON_BIN="$CORE_DIR/target/release/sandkilnd"

PID_FILE="${SANDKILN_CTL_PID_FILE:-/tmp/sandkiln-daemon.pid}"
LOG_FILE="${SANDKILN_CTL_LOG_FILE:-/tmp/sandkiln-daemon.log}"
HEALTH_URL="http://${SANDKILN_LISTEN_ADDR:-127.0.0.1:7777}/healthz"

# ---------------------------------------------------------------------------
running_pid() {
  # Prints the PID if the daemon is actually running (PID file exists,
  # names a live process, and that process really is sandkilnd — not just
  # whatever unrelated process happens to now own a stale, reused PID).
  # Prints nothing and returns non-zero otherwise.
  [ -f "$PID_FILE" ] || return 1
  local pid
  pid="$(cat "$PID_FILE" 2>/dev/null)"
  [ -n "$pid" ] || return 1
  kill -0 "$pid" 2>/dev/null || return 1
  case "$(ps -p "$pid" -o comm= 2>/dev/null)" in
    sandkilnd*) echo "$pid"; return 0 ;;
    *) return 1 ;;
  esac
}

wait_for_health() {
  local deadline=$((SECONDS + 15))
  while [ "$SECONDS" -lt "$deadline" ]; do
    curl -sf -o /dev/null "$HEALTH_URL" && return 0
    sleep 0.3
  done
  return 1
}

# ---------------------------------------------------------------------------
cmd_start() {
  local do_build=1 do_preflight=1
  for arg in "$@"; do
    case "$arg" in
      --no-build) do_build=0 ;;
      --no-preflight) do_preflight=0 ;;
      *) echo "start: unknown option $arg" >&2; exit 2 ;;
    esac
  done

  if pid="$(running_pid)"; then
    echo "sandkilnd already running (pid $pid) — nothing to do. Use 'restart' to cycle it."
    return 0
  fi
  # A stale PID file (process gone, file left behind) shouldn't block a
  # fresh start — running_pid already validated it above; if we're here,
  # whatever's in it is dead weight.
  rm -f "$PID_FILE"

  if [ "$do_build" -eq 1 ]; then
    echo "==> building sandkilnd (cargo build --release; incremental, fast if nothing changed)"
    if ! (cd "$CORE_DIR" && cargo build --release --workspace); then
      echo "build failed — not starting" >&2
      exit 1
    fi
  fi

  if [ ! -x "$DAEMON_BIN" ]; then
    echo "no daemon binary at $DAEMON_BIN — run without --no-build, or build it yourself first" >&2
    exit 1
  fi

  if [ "$do_preflight" -eq 1 ] && [ -x "$SCRIPT_DIR/preflight-check.sh" ]; then
    echo "==> preflight check"
    if ! "$SCRIPT_DIR/preflight-check.sh" --daemon-bin "$DAEMON_BIN"; then
      echo "preflight check failed — fix the FAIL items above, or re-run with --no-preflight to start anyway" >&2
      exit 1
    fi
  fi

  if ! getcap "$DAEMON_BIN" 2>/dev/null | grep -q cap_net_admin; then
    echo "==> $DAEMON_BIN has no CAP_NET_ADMIN — granting it (needs sudo)"
    if ! sudo bash "$SCRIPT_DIR/grant-net-admin.sh" "$DAEMON_BIN"; then
      echo "failed to grant CAP_NET_ADMIN — see scripts/grant-net-admin.sh" >&2
      exit 1
    fi
  fi

  echo "==> starting sandkilnd (log: $LOG_FILE)"
  nohup "$DAEMON_BIN" >"$LOG_FILE" 2>&1 < /dev/null &
  local pid=$!
  disown "$pid" 2>/dev/null || true
  echo "$pid" > "$PID_FILE"

  if wait_for_health; then
    echo "sandkilnd is up (pid $pid): $HEALTH_URL"
  else
    echo "sandkilnd did not become healthy within 15s — check $LOG_FILE" >&2
    echo "--- tail $LOG_FILE ---" >&2
    tail -n 30 "$LOG_FILE" >&2 2>/dev/null
    exit 1
  fi
}

cmd_stop() {
  local pid
  if ! pid="$(running_pid)"; then
    echo "sandkilnd is not running"
    rm -f "$PID_FILE"
    return 0
  fi

  echo "==> stopping sandkilnd (pid $pid)"
  kill "$pid" 2>/dev/null
  local deadline=$((SECONDS + 10))
  while kill -0 "$pid" 2>/dev/null; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "still running after 10s — sending SIGKILL" >&2
      kill -9 "$pid" 2>/dev/null
      break
    fi
    sleep 0.2
  done
  rm -f "$PID_FILE"

  # Best-effort: reap any Firecracker processes this daemon spawned and
  # left behind. Scoped to this project's own api-socket path convention
  # (/tmp/sandkiln-fc-*.sock) rather than every "firecracker" process on
  # the host, so this can't touch another user's or another tool's VMs on
  # a shared box.
  local orphans
  orphans="$(pgrep -f 'firecracker --api-sock /tmp/sandkiln-fc-' 2>/dev/null || true)"
  if [ -n "$orphans" ]; then
    echo "==> cleaning up $(echo "$orphans" | wc -l) orphaned firecracker process(es) sandkilnd left running"
    echo "$orphans" | xargs -r kill -9 2>/dev/null
  fi

  echo "sandkilnd stopped"
}

cmd_restart() {
  cmd_stop
  cmd_start "$@"
}

cmd_status() {
  if pid="$(running_pid)"; then
    echo "sandkilnd: running (pid $pid)"
    if curl -sf -o /dev/null "$HEALTH_URL"; then
      echo "healthz: ok ($HEALTH_URL)"
    else
      echo "healthz: not responding — the process is up but not answering, check $LOG_FILE" >&2
      exit 1
    fi
  else
    echo "sandkilnd: not running"
    exit 1
  fi
}

cmd_logs() {
  [ -f "$LOG_FILE" ] || { echo "no log file yet at $LOG_FILE — has it been started?" >&2; exit 1; }
  if [ "${1:-}" = "-f" ]; then
    tail -n 50 -f "$LOG_FILE"
  else
    tail -n 50 "$LOG_FILE"
  fi
}

# ---------------------------------------------------------------------------
case "${1:-}" in
  start) shift; cmd_start "$@" ;;
  stop) shift; cmd_stop ;;
  restart) shift; cmd_restart "$@" ;;
  status) cmd_status ;;
  logs) shift; cmd_logs "${1:-}" ;;
  *)
    echo "usage: $0 {start [--no-build] [--no-preflight] | stop | restart [--no-build] [--no-preflight] | status | logs [-f]}" >&2
    exit 2
    ;;
esac

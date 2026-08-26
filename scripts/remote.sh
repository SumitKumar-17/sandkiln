#!/usr/bin/env bash
# Sync this repo to the remote dev box and (optionally) run a command there.
# The remote box is where anything needing KVM / a real Linux toolchain runs.
#
# Usage:
#   scripts/remote.sh sync              # push local repo -> remote
#   scripts/remote.sh run <command...>  # sync, then run <command> in the remote repo dir
#   scripts/remote.sh ssh               # open an interactive shell in the remote repo dir

set -euo pipefail

REMOTE_HOST="${SANDKILN_REMOTE_HOST:-t1000@10.5.31.157}"
REMOTE_DIR="${SANDKILN_REMOTE_DIR:-~/sandkiln}"
LOCAL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

sync() {
  ssh "$REMOTE_HOST" "mkdir -p $REMOTE_DIR"
  tar -C "$LOCAL_DIR" -czf - \
    --exclude='.git' \
    --exclude='target' \
    --exclude='node_modules' \
    --exclude='dist' \
    --exclude='images/build' \
    . | ssh "$REMOTE_HOST" "mkdir -p $REMOTE_DIR && tar -C $REMOTE_DIR -xzf -"
}

case "${1:-}" in
  sync)
    sync
    ;;
  run)
    shift
    sync
    ssh "$REMOTE_HOST" "cd $REMOTE_DIR && $*"
    ;;
  ssh)
    sync
    ssh -t "$REMOTE_HOST" "cd $REMOTE_DIR && exec \$SHELL -l"
    ;;
  *)
    echo "usage: $0 {sync|run <command...>|ssh}" >&2
    exit 1
    ;;
esac

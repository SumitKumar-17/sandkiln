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

# Neither cargo nor a current Node.js is on PATH in a non-interactive SSH
# shell (both are wired up by lines sourced from .bashrc, which only a
# login/interactive shell picks up) — see root AGENTS.md's "Development
# Environment" gotchas. Every `run`/`ssh` command below is prefixed with
# this so a plain `scripts/remote.sh run npm test` or `cargo build` just
# works, instead of silently hitting a stale system Node or a missing
# cargo.
REMOTE_ENV_PRELUDE='[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"; export NVM_DIR="$HOME/.nvm"; [ -s "$NVM_DIR/nvm.sh" ] && . "$NVM_DIR/nvm.sh"'

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
    ssh "$REMOTE_HOST" "$REMOTE_ENV_PRELUDE; cd $REMOTE_DIR && $*"
    ;;
  ssh)
    sync
    ssh -t "$REMOTE_HOST" "$REMOTE_ENV_PRELUDE; cd $REMOTE_DIR && exec \$SHELL -l"
    ;;
  *)
    echo "usage: $0 {sync|run <command...>|ssh}" >&2
    exit 1
    ;;
esac

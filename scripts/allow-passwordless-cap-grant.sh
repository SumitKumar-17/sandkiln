#!/usr/bin/env bash
# One-time dev convenience: lets `scripts/sandkilnd-ctl.sh restart` grant
# CAP_NET_ADMIN to the freshly-rebuilt binary without an interactive sudo
# prompt every single time — setcap doesn't survive a binary being
# replaced, so every rebuild-then-restart otherwise needs a password typed
# somewhere sudo can actually read it from (which a non-interactive SSH
# session, e.g. via `scripts/remote.sh run`, can't provide at all — see
# root AGENTS.md's SSH/non-interactive-shell gotchas).
#
# Adds a single, narrowly-scoped NOPASSWD rule: only this exact daemon
# binary path, only via scripts/grant-net-admin.sh, nothing broader.
# Review the generated line before running this if you have any doubt —
# it's one line, printed below before it's written.
#
# Usage: sudo scripts/allow-passwordless-cap-grant.sh [path-to-sandkilnd-binary]
#   (defaults to core/target/release/sandkilnd relative to the repo root)

set -euo pipefail

[ "$(id -u)" -eq 0 ] || { echo "must run as root: sudo $0 [path-to-sandkilnd-binary]" >&2; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BIN="${1:-$REPO_ROOT/core/target/release/sandkilnd}"
GRANT_SCRIPT="$SCRIPT_DIR/grant-net-admin.sh"
BASH_BIN="$(command -v bash)"
# The user who invoked sudo, not root — that's who actually runs
# sandkilnd-ctl.sh day to day and needs the passwordless rule.
TARGET_USER="${SUDO_USER:?run this via sudo, not as root directly, so the real user is known}"

# sudoers matches the exact command line, and sandkilnd-ctl.sh invokes
# this as `sudo bash grant-net-admin.sh <bin>` (so the "command" sudo
# actually sees is bash, not the script) — a rule naming just the script
# path silently never matches and falls back to a password prompt. Cover
# both that invocation and a direct `sudo grant-net-admin.sh <bin>` call.
RULE="$TARGET_USER ALL=(root) NOPASSWD: $BASH_BIN $GRANT_SCRIPT $BIN, $GRANT_SCRIPT $BIN"
RULE_FILE="/etc/sudoers.d/sandkiln-cap-grant"

echo "About to write this sudoers rule to $RULE_FILE:"
echo "  $RULE"
echo
read -r -p "Proceed? [y/N] " confirm
case "$confirm" in
  y|Y) ;;
  *) echo "aborted, nothing written"; exit 1 ;;
esac

echo "$RULE" > "$RULE_FILE"
chmod 0440 "$RULE_FILE"
visudo -c -f "$RULE_FILE" || { echo "generated rule failed sudoers syntax check — removing it" >&2; rm -f "$RULE_FILE"; exit 1; }

echo "done — '$TARGET_USER' can now run 'sudo $GRANT_SCRIPT $BIN' without a password."
echo "sandkilnd-ctl.sh restart will pick this up automatically on its next run."

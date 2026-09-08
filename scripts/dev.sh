#!/usr/bin/env bash
# One command in front of every other script in this directory — a
# dev-loop shortcut, not a replacement for any of them (each still works
# standalone with its own full flag set; this just saves remembering
# which file does what). Every subcommand below is a thin pass-through:
# it execs the real script with whatever arguments follow the subcommand
# name, so `scripts/dev.sh preflight --root-checks` is exactly
# `scripts/preflight-check.sh --root-checks`, nothing hidden.
#
# Usage: scripts/dev.sh <subcommand> [args...]
#
# Host setup:
#   setup [--production] ...    scripts/setup.sh
#   preflight [...]             scripts/preflight-check.sh
#
# Daemon lifecycle:
#   start|stop|restart|status|logs [...]   scripts/sandkilnd-ctl.sh <same>
#
# Build and test (Rust workspace, in core/):
#   build [cargo args...]       cargo build --release --workspace
#   check [cargo args...]       cargo build --release --workspace (compile-only sanity, same as build)
#   unit-test [cargo args...]   cargo test --workspace
#   bench                       cargo bench -p sandkiln-vmm --bench vm_lifecycle
#                                (SANDKILN_BENCH_FIRECRACKER_BIN/_KERNEL_PATH/_ROOTFS_PATH
#                                 default to the standard ~/sandkiln-tools layout if unset —
#                                 override any of them to point elsewhere)
#
# Against a running daemon:
#   integration-test [base-url]                       scripts/integration-test.sh
#   load-test [concurrency] [iterations] [base-url]    scripts/load-test.sh
#
# Everything else is intentionally NOT wrapped here — one-time or
# narrowly-scoped steps `setup.sh` already calls in the right order; run
# them directly if you need one in isolation:
#   scripts/host-setup/   create-tap-pool, grant-net-admin,
#                         install-firecracker, start-dns-proxy,
#                         allow-passwordless-cap-grant
#   scripts/dev-tools/    boot-test-vm, setup-tap-network (a narrower,
#                         older single-tap networking model — not part
#                         of a normal setup, see their own comments)
#   scripts/install-systemd-service.sh   persistent, boot-surviving
#                                         deployment (vs. sandkilnd-ctl's
#                                         direct-run dev loop above)
#
# Run with no arguments (or `help`/`-h`/`--help`) to print this again.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

usage() {
  # Prints this file's own leading comment block (everything from line 2
  # up to the first non-comment line), so the usage text and this
  # function can never drift apart the way a hardcoded line range would.
  awk 'NR==1 {next} /^#/ {sub(/^# ?/, ""); print; next} {exit}' "${BASH_SOURCE[0]}"
}

# Sensible defaults matching scripts/setup.sh's own layout — only applied
# when the caller hasn't already set these, so an explicit override (e.g.
# a second kernel/rootfs you're bisecting against) always wins.
default_bench_env() {
  export SANDKILN_BENCH_FIRECRACKER_BIN="${SANDKILN_BENCH_FIRECRACKER_BIN:-$HOME/sandkiln-tools/bin/firecracker}"
  export SANDKILN_BENCH_KERNEL_PATH="${SANDKILN_BENCH_KERNEL_PATH:-$HOME/sandkiln-tools/images/vmlinux-5.10.223}"
  export SANDKILN_BENCH_ROOTFS_PATH="${SANDKILN_BENCH_ROOTFS_PATH:-$HOME/sandkiln-tools/images/sandkiln-base.ext4}"
}

subcommand="${1:-help}"
[ $# -gt 0 ] && shift

case "$subcommand" in
  setup)
    exec "$SCRIPT_DIR/setup.sh" "$@"
    ;;
  preflight)
    exec "$SCRIPT_DIR/preflight-check.sh" "$@"
    ;;
  start | stop | restart | status | logs)
    exec "$SCRIPT_DIR/sandkilnd-ctl.sh" "$subcommand" "$@"
    ;;
  build | check)
    exec cargo build --release --manifest-path "$REPO_ROOT/core/Cargo.toml" --workspace "$@"
    ;;
  unit-test)
    exec cargo test --manifest-path "$REPO_ROOT/core/Cargo.toml" --workspace "$@"
    ;;
  bench)
    default_bench_env
    exec cargo bench --manifest-path "$REPO_ROOT/core/Cargo.toml" -p sandkiln-vmm --bench vm_lifecycle "$@"
    ;;
  integration-test)
    exec "$SCRIPT_DIR/integration-test.sh" "$@"
    ;;
  load-test)
    exec "$SCRIPT_DIR/load-test.sh" "$@"
    ;;
  help | -h | --help)
    usage
    ;;
  *)
    echo "unknown subcommand: $subcommand" >&2
    echo >&2
    usage >&2
    exit 1
    ;;
esac

#!/usr/bin/env bash
# Logic behind the sandkiln-run composite action. Reads its inputs the way a
# GitHub action receives them (INPUT_<NAME> env vars, hyphens kept) and talks
# to the daemon only through the `kiln` CLI.
#
# Usage:
#   run.sh run       create a sandbox, upload the workspace, run the command,
#                    write sandbox-id and exit-code to $GITHUB_OUTPUT
#   run.sh cleanup   destroy the sandbox recorded by `run` (idempotent)
#
# Inputs: daemon-url, auth-token, command, image, working-directory, egress,
# allow-cidrs, timeout, keep-on-failure, exclude. Environment: GITHUB_WORKSPACE,
# GITHUB_OUTPUT, RUNNER_TEMP, GITHUB_RUN_ID, GITHUB_REPOSITORY (all set by
# GitHub). Exit codes of `run`: the command's own, 124 on timeout, 125 when the
# exit code could not be read back, 2 on bad input or setup failure.

set -euo pipefail

GUEST_DIR=/workspace
GUEST_ARCHIVE=/tmp/sandkiln-workspace.tgz
GUEST_SCRIPT=/tmp/sandkiln-job.sh
GUEST_EXIT_FILE=/tmp/sandkiln-exit-code

die() { echo "::error::sandkiln-run: $*" >&2; exit 2; }
input() { printenv "INPUT_${1^^}" 2>/dev/null || true; }

state_dir() { echo "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/sandkiln-run-${GITHUB_RUN_ID:-local}"; }
set_output() { [ -n "${GITHUB_OUTPUT:-}" ] && printf '%s=%s\n' "$1" "$2" >>"$GITHUB_OUTPUT" || true; }

configure() {
  local url token
  url=$(input daemon-url)
  token=$(input auth-token)
  [ -n "$url" ] || die "daemon-url is required"
  case "$url" in
    https://*) ;;
    http://127.0.0.1*|http://localhost*|http://\[::1\]*) ;;
    *) die "daemon-url must be https:// (plain http is only accepted for loopback)" ;;
  esac
  if [ -n "$token" ]; then
    echo "::add-mask::$token"
    export SANDKILN_AUTH_TOKEN="$token"
  elif [[ "$url" == https://* ]]; then
    echo "::warning::sandkiln-run: no auth-token given for a remote daemon; it should never be exposed without one"
  fi
  export SANDKILN_DAEMON_URL="$url"
  command -v kiln >/dev/null || die "the kiln CLI is not on PATH"
}

egress_args() {
  local mode cidrs c
  mode=$(input egress)
  cidrs=$(input allow-cidrs)
  case "$mode" in
    ""|default) return 0 ;;
    deny_all|allow_all) ;;
    *) die "egress must be one of: default, deny_all, allow_all (got '$mode')" ;;
  esac
  printf '%s\n' --egress-mode "$mode"
  for c in ${cidrs//,/ }; do printf '%s\n' --allow-cidr "$c"; done
}

upload_workspace() {
  local id=$1 src=$2 archive=$3 exclude e
  local -a tar_args=(--exclude=.git)
  exclude=$(input exclude)
  # .git is always excluded: a checkout can leave a credential in .git/config,
  # which must never reach the sandbox.
  for e in ${exclude//,/ }; do tar_args+=("--exclude=$e"); done
  # Explicit returns: the caller uses `||`, which switches `set -e` off here.
  tar -czf "$archive" "${tar_args[@]}" -C "$src" . || return 1
  echo "uploading $(du -h "$archive" | cut -f1) workspace archive"
  kiln sandbox exec "$id" -- mkdir -p "$GUEST_DIR" >/dev/null || return 1
  kiln sandbox write "$id" "$GUEST_ARCHIVE" "$archive" >/dev/null || return 1
  kiln sandbox exec "$id" -- tar -xzf "$GUEST_ARCHIVE" -C "$GUEST_DIR" || return 1
}

cmd_run() {
  configure
  local command timeout workdir src state id rc=0 name egress
  command=$(input command)
  [ -n "$command" ] || die "command is required"
  timeout=$(input timeout)
  timeout=${timeout:-600}
  [[ "$timeout" =~ ^[1-9][0-9]*$ ]] || die "timeout must be a positive number of seconds"
  workdir=$(input working-directory)
  src="${GITHUB_WORKSPACE:-$PWD}/${workdir:-.}"
  [ -d "$src" ] || die "working-directory '$workdir' does not exist in the workspace"
  state=$(state_dir)
  mkdir -p "$state"
  chmod 700 "$state"

  local -a create=(sandbox create --tag "github-run=${GITHUB_RUN_ID:-local}" --tag "github-repo=${GITHUB_REPOSITORY:-local}")
  [ -z "$(input image)" ] || create+=(--image "$(input image)")
  # Assigned first: `die` inside a process substitution would not stop the run.
  egress=$(egress_args)
  while IFS= read -r name; do [ -z "$name" ] || create+=("$name"); done <<<"$egress"

  id=$(kiln "${create[@]}") || die "could not create a sandbox"
  # Recorded before anything else can fail, so cleanup always knows the id.
  printf '%s\n' "$id" >"$state/sandbox-id"
  printf '%s\n' "$(input keep-on-failure)" >"$state/keep-on-failure"
  set_output sandbox-id "$id"
  echo "sandbox $id created"

  upload_workspace "$id" "$src" "$state/workspace.tgz" || die "could not upload the workspace"
  printf '%s\n' "$command" >"$state/job.sh"
  kiln sandbox write "$id" "$GUEST_SCRIPT" "$state/job.sh" >/dev/null || die "could not upload the command"

  # The exit code is also written to a file in the guest: the streaming client
  # reports 0 when its connection drops, which must never read as success.
  # The limit is enforced in the guest with `timeout` (it signals the command's
  # whole process group, then SIGKILLs it) so a timed-out job really stops; the
  # streaming API has no timeout of its own. An image without `timeout` falls
  # back to the client-side limit below, which only abandons the stream.
  local run_job="if command -v timeout >/dev/null 2>&1; then timeout --kill-after=10 $timeout sh $GUEST_SCRIPT; else sh $GUEST_SCRIPT; fi"
  local wrapper="cd $GUEST_DIR && $run_job; rc=\$?; echo \$rc >$GUEST_EXIT_FILE; exit \$rc"
  echo "running command (timeout ${timeout}s)"
  # The backstop outlasts the guest's own limit plus its kill delay, so the
  # guest-side exit code (124) is what normally reports a timeout.
  timeout --kill-after=10 "$((timeout + 30))" kiln sandbox exec-stream "$id" -- sh -c "$wrapper" || rc=$?

  if [ "$rc" -eq 124 ] || [ "$rc" -eq 137 ]; then
    echo "::error::sandkiln-run: command timed out after ${timeout}s"
    rc=124
  else
    local reported
    reported=$(kiln sandbox read "$id" "$GUEST_EXIT_FILE" 2>/dev/null | tr -d '[:space:]') || reported=""
    if [[ "$reported" =~ ^[0-9]+$ ]]; then
      rc=$reported
      # `timeout` exits 124, or 137 after its SIGKILL: the job was killed in the guest.
      if [ "$rc" -eq 124 ] || [ "$rc" -eq 137 ]; then
        echo "::error::sandkiln-run: command timed out after ${timeout}s"
        rc=124
      fi
    else
      echo "::error::sandkiln-run: the sandbox never reported an exit code (stream dropped?)"
      rc=125
    fi
  fi
  printf '%s\n' "$rc" >"$state/exit-code"
  set_output exit-code "$rc"
  [ "$rc" -eq 0 ] || echo "::error::sandkiln-run: command exited with code $rc"
  return "$rc"
}

cmd_cleanup() {
  local state id keep rc=0
  state=$(state_dir)
  [ -f "$state/sandbox-id" ] || { echo "no sandbox recorded; nothing to clean up"; return 0; }
  id=$(<"$state/sandbox-id")
  keep=$(<"$state/keep-on-failure")
  rc=$(cat "$state/exit-code" 2>/dev/null || echo 125)
  if [ "$keep" = "true" ] && [ "$rc" -ne 0 ]; then
    echo "::warning::sandkiln-run: keep-on-failure is set, sandbox $id was left running; remove it with: kiln sandbox rm $id --destroy"
    return 0
  fi
  configure
  if kiln sandbox rm "$id" --destroy; then
    rm -rf "$state"
  else
    echo "::error::sandkiln-run: could not destroy sandbox $id; remove it with: kiln sandbox rm $id --destroy"
    return 1
  fi
}

case "${1:-}" in
  run) cmd_run ;;
  cleanup) cmd_cleanup ;;
  *) echo "usage: run.sh run|cleanup" >&2; exit 2 ;;
esac

# Reads the daemon's effective configuration from the daemon itself
# (`sandkilnd --print-config`), so scripts never carry their own copy of the
# defaults. Source this file; it defines:
#
#   load_daemon_config <sandkilnd-binary> [config-file]
#       Fills the DAEMON_CFG associative array (key -> value). Returns 1 if the
#       binary is missing or its configuration is invalid (e.g. no binary built
#       yet); callers then fall back to their own literals.
#   cfg <key> [fallback]
#       Prints the value, or the fallback when unavailable or unset.
#   find_config_file
#       Prints the config file scripts should hand to the daemon: $SANDKILN_CONFIG,
#       else <repo>/sandkiln.toml if it exists, else nothing.

declare -gA DAEMON_CFG=()

load_daemon_config() {
  local bin="$1" file="${2:-}" out line key rest val
  [ -x "$bin" ] || return 1
  local args=()
  [ -n "$file" ] && args=(--config "$file")
  out="$("$bin" "${args[@]}" --print-config 2>/dev/null)" || return 1
  DAEMON_CFG=()
  while IFS= read -r line; do
    case "$line" in '#'*|'') continue ;; esac
    key="${line%% = *}"
    rest="${line#* = }"
    val="${rest%%  \#*}"
    val="${val#\"}"; val="${val%\"}"
    [ "$val" = "unset" ] && val=""
    DAEMON_CFG["$key"]="$val"
  done <<< "$out"
}

cfg() {
  local v="${DAEMON_CFG[$1]:-}"
  [ -n "$v" ] && echo "$v" || echo "${2:-}"
}

find_config_file() {
  local root
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  if [ -n "${SANDKILN_CONFIG:-}" ]; then
    echo "$SANDKILN_CONFIG"
  elif [ -f "$root/sandkiln.toml" ]; then
    echo "$root/sandkiln.toml"
  fi
}

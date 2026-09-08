# Requires the guest agent baked into the daemon's base rootfs to be
# rebuilt and re-injected after these ops were added (images/inject-
# agent.sh, or a fresh scripts/setup.sh run) -- against a stale agent
# every check below fails with "unknown variant" from the agent's own
# (older) protocol crate, not a bug in this suite. See CHANGELOG.md.
section "filesystem operations: chmod, chown, mkdir, rename, copy, symlink, readlink, truncate, list-dir"
status="$(req POST /sandboxes '{"tags":{"suite":"integration","case":"fs-ops"}}')"
assert_status "create sandbox for filesystem ops test" 200 "$status"
SBX_FS="$(extract id < "$WORKDIR/resp.json")"
if [ -z "$SBX_FS" ]; then
  fail "create sandbox for filesystem ops test returned no id — aborting filesystem ops checks"
else
  CREATED_SANDBOXES+=("$SBX_FS")

  status="$(req POST "/sandboxes/$SBX_FS/mkdir" '{"path":"/tmp/fsops/nested","parents":true}')"
  assert_status "mkdir -p creates nested directories in one call" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/mkdir" '{"path":"/tmp/fsops/nested"}')"
  assert_status "mkdir without parents on an already-existing directory is rejected" 400 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/write-file" '{"path":"/tmp/fsops/a.txt","content_base64":"aGVsbG8="}')"
  assert_status "write-file for the fs-ops fixture" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/chmod" '{"path":"/tmp/fsops/a.txt","mode":420}')"
  assert_status "chmod to 0644 (420 decimal)" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/list-dir" '{"path":"/tmp/fsops"}')"
  assert_status "list-dir on the fs-ops fixture directory" 200 "$status"
  list_body="$(cat "$WORKDIR/resp.json")"
  assert_contains "listing includes a.txt" "$list_body" '"name":"a.txt"'
  assert_contains "listing includes the nested subdirectory" "$list_body" '"name":"nested"'
  assert_contains "listing reports a.txt's chmod'd mode" "$list_body" '"mode":420'
  assert_contains "listing marks nested as a directory" "$list_body" '"is_dir":true'

  status="$(req POST "/sandboxes/$SBX_FS/rename" '{"from":"/tmp/fsops/a.txt","to":"/tmp/fsops/b.txt"}')"
  assert_status "rename a.txt to b.txt" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/read-file" '{"path":"/tmp/fsops/b.txt"}')"
  assert_status "read-file confirms the rename actually moved the file" 200 "$status"
  assert_contains "renamed file kept its content" "$(cat "$WORKDIR/resp.json")" "aGVsbG8="

  status="$(req POST "/sandboxes/$SBX_FS/copy" '{"from":"/tmp/fsops/b.txt","to":"/tmp/fsops/c.txt"}')"
  assert_status "copy b.txt to c.txt" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/read-file" '{"path":"/tmp/fsops/b.txt"}')"
  assert_status "the original (b.txt) still exists after copy, unlike rename" 200 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/symlink" '{"target":"/tmp/fsops/c.txt","link_path":"/tmp/fsops/c-link.txt"}')"
  assert_status "create a symlink to c.txt" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/readlink" '{"path":"/tmp/fsops/c-link.txt"}')"
  assert_status "readlink reports the symlink target" 200 "$status"
  assert_contains "readlink target is exactly what was passed to symlink" "$(cat "$WORKDIR/resp.json")" '"target":"/tmp/fsops/c.txt"'

  status="$(req POST "/sandboxes/$SBX_FS/list-dir" '{"path":"/tmp/fsops"}')"
  assert_contains "listing marks c-link.txt as a symlink" "$(cat "$WORKDIR/resp.json")" '"name":"c-link.txt","is_dir":false,"is_symlink":true'

  status="$(req POST "/sandboxes/$SBX_FS/truncate" '{"path":"/tmp/fsops/c.txt","size":2}')"
  assert_status "truncate c.txt down to 2 bytes" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/read-file" '{"path":"/tmp/fsops/c.txt"}')"
  assert_status "read-file after truncate" 200 "$status"
  assert_contains "truncated content is exactly the first 2 bytes ('he' -> base64 aGU=)" "$(cat "$WORKDIR/resp.json")" '"content_base64":"aGU="'

  status="$(req POST "/sandboxes/$SBX_FS/chown" '{"path":"/tmp/fsops/c.txt","uid":0,"gid":0}')"
  assert_status "chown c.txt to root:root (agent runs as root inside the guest)" 204 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/chmod" '{"path":"/tmp/fsops/does-not-exist","mode":420}')"
  assert_status "chmod on a nonexistent path is rejected" 400 "$status"

  status="$(req POST "/sandboxes/$SBX_FS/readlink" '{"path":"/tmp/fsops/b.txt"}')"
  assert_status "readlink on a non-symlink path is rejected" 400 "$status"

  status="$(req DELETE "/sandboxes/$SBX_FS?keep=false")"
  assert_status "stop the filesystem-ops sandbox" 204 "$status"
  CREATED_SANDBOXES=("${CREATED_SANDBOXES[@]/$SBX_FS}")
fi

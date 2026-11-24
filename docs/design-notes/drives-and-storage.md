# Drives and remote storage

Drives are persistent filesystem storage that outlives a sandbox and can be
reattached to a new one.

## Read-only shared drives

A drive attached read-only may be attached to any number of sandboxes at
once. A read-write attachment, existing or requested, still needs a single
exclusive holder. `AppState::drive_holders()` tracks every holder and
whether each is read-only; `can_attach_read_only()` is the pure rule. It
covers held snapshots as well as live sandboxes.

## SDK and CLI

`Drive.create/list/delete` and `drives`/`DriveAttachment` on
`Sandbox.create()` in both SDKs; `kiln drive create|ls|rm` and
`--drive <id[:ro]>` on `kiln sandbox create`.

## Remote storage mounts

`POST/GET/DELETE /sandboxes/:id/mounts` mounts an S3-compatible bucket via
`rclone mount` running entirely inside the guest. No new wire protocol: it
chains the existing `Mkdir`/`WriteFile`/`Chmod`/`Exec` requests (see
`routes_mounts.rs`).

Prerequisites a stock setup lacks:
- A guest kernel with `CONFIG_FUSE_FS` (Firecracker's default configs omit
  it and guest kernels cannot load modules): `images/build-guest-kernel.sh`.
- `rclone` and `fusermount3` in the rootfs:
  `images/inject-rclone.sh`, `scripts/dev.sh inject-rclone`. See
  `SELF_HOSTING.md`, "Remote storage mounts (optional)".

Credentials reach the guest as a `0600` rclone config via
`WriteFile`+`Chmod`, never as a command-line argument. There is no
re-application on resume, fork or restore: a mount is a live guest FUSE
process captured by the Firecracker snapshot with the rest of memory.

SDK and CLI: `Sandbox.mount()/listMounts()/unmount()` (JS/TS),
`mount()/list_mounts()/unmount()` (Python), `kiln sandbox
mount|mounts|unmount`. Verified live for the empty-list case and a clean
`SandkilnApiError` against an unreachable endpoint. A full success needs a
real S3-compatible endpoint the dev box lacks (same caveat as
`scripts/integration-tests/22-mounts.sh`).

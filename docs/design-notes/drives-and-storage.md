# Drives and remote storage

Drives are persistent filesystem storage that outlives a sandbox and can be
reattached to a new one.

## Read-only shared drives

A drive attached read-only may be attached to any number of sandboxes at
once; a read-write attachment still needs a single exclusive holder. The
rule (`can_attach_read_only()`, `AppState::drive_holders()`) is on the
[drives internals page](../../docs-site/internals/drives.mdx); it covers held
snapshots as well as live sandboxes, and any read-write case behaves exactly
as before read-only sharing existed.

## SDK and CLI

`Drive.create/list/delete` and `drives`/`DriveAttachment` on
`Sandbox.create()` in both SDKs; `kiln drive create|ls|rm` and
`--drive <id[:ro]>` on `kiln sandbox create`.

## Remote storage mounts

`POST/GET/DELETE /sandboxes/:id/mounts` mounts an S3-compatible bucket via
`rclone mount` running entirely inside the guest, with no new wire protocol
(see `routes_mounts.rs` and the
[FUSE and rclone internals page](../../docs-site/internals/fuse-rclone-mounts.mdx),
which covers the chained requests, the `0600` credentials file, the
`mountpoint -q` verification and why nothing is re-applied on resume).

Prerequisites a stock setup lacks: a guest kernel with `CONFIG_FUSE_FS`
(`images/build-guest-kernel.sh`) and `rclone`/`fusermount3` in the rootfs
(`images/inject-rclone.sh`, `scripts/dev.sh inject-rclone`). See
`SELF_HOSTING.md`, "Remote storage mounts (optional)".

SDK and CLI: `Sandbox.mount()/listMounts()/unmount()` (JS/TS),
`mount()/list_mounts()/unmount()` (Python), `kiln sandbox
mount|mounts|unmount`. Mounts were daemon-HTTP-only for a while: `examples/remote-storage-mount`
called the three routes with raw `fetch()` until the SDK/CLI surface landed.
Verified live against a real daemon for the empty-list case (`listMounts()`
on a mount-free sandbox) and a clean
`SandkilnApiError` against an unreachable endpoint. A full success needs a
real S3-compatible endpoint the dev box lacks (same caveat as
`scripts/integration-tests/22-mounts.sh`).

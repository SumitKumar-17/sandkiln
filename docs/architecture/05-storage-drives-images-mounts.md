# Drives, managed images, and remote storage mounts

## Drives — persistent block storage attached to a VM

A drive is a real ext4 file attached to a Firecracker VM as a block device,
outliving any single sandbox.

- **Ownership tracking, centrally.** `AppState::drive_holders(drive_id)`
  answers "who currently holds this drive" across *both* live sandboxes and
  held snapshots in one place — this is the exact mechanism that prevented a
  real data-corruption class of bug (two VMs double-attaching the same
  writable drive).
- **Read-only sharing** is explicitly allowed to stack: multiple holders can
  attach the same drive read-only at once; a write-mode attach is exclusive.
  See `can_attach_read_only_allows_stacking_more_read_only_holders` in
  `state.rs` for the exact rule as a test, not just prose.
- **Path-traversal rejected at the daemon boundary, not just client-side.**
  `drive::validate_id` (`sandkiln-vmm`) rejects anything but
  alphanumeric/hyphen/underscore, length-capped — `../etc/passwd`-style ids
  fail before ever touching the filesystem. Same validation function pattern
  reused for image ids.

## Managed images

`routes_images.rs` (`POST/GET/DELETE /images`) lets a caller **register** an
already-built ext4 rootfs file under a name via `sandkiln_vmm::image::ImageStore::register`,
then boot a specific sandbox from it via `image_id` without touching the
daemon's global default rootfs (`SANDKILN_BASE_ROOTFS`).

- Registration **copies** the file into `SANDKILN_IMAGES_DIR` — it does not
  accept an HTTP upload and does not build or convert anything. The file must
  already be a real rootfs with the guest agent already injected
  (`images/inject-agent.sh`), built out of band.
- The daemon has no way to verify that precondition itself (no root to
  loop-mount) — `scripts/preflight-check.sh --root-checks --rootfs-image <path>`
  is the way to check before registering.
- **Converting a Docker/OCI image into a bootable rootfs is explicitly not
  attempted** by this mechanism — a genuinely separate, larger problem, kept
  honest in `ROADMAP.md` rather than silently unsupported.

## Remote storage mounts (FUSE + rclone)

`routes_mounts.rs` (`POST/GET/DELETE /sandboxes/:id/mounts`) mounts an
S3-compatible bucket *inside* a sandbox via `rclone mount`, using Linux FUSE.

- Needs a guest kernel built with `CONFIG_FUSE_FS` (Firecracker's default
  kernel configs don't set it — `images/build-guest-kernel.sh`) and a rootfs
  with `rclone` + `fusermount3` baked in (`images/inject-rclone.sh`).
- `fusermount3` is required even though everything inside the guest runs as
  root: rclone's Linux FUSE backend always execs `fusermount3` to do the
  actual mount — there's no direct `mount(2)` fallback for root. It only
  depends on libc, so it's copyable straight from any Debian/Ubuntu build
  host, no `libfuse3` needed.
- Exposed in both SDKs (`sandbox.mount()`/`.listMounts()`/`.unmount()` /
  `mount()`/`list_mounts()`/`unmount()`) and the CLI (`kiln sandbox mount|mounts|unmount`).

## Status

Drives and images: done, live-verified. Mounts: code path complete and SDK/CLI-exposed;
clean-failure behavior verified live, but full success against a real S3-compatible
endpoint hasn't been verified on the dev box (no such endpoint available there) —
stated honestly rather than claimed. See [`website/src/content/docs/internals/drives.md`](../../website/src/content/docs/internals/drives.md),
[`images.md`](../../website/src/content/docs/internals/images.md), and
[`fuse-rclone-mounts.md`](../../website/src/content/docs/internals/fuse-rclone-mounts.md).

# AGENTS.md: images/

Read root `AGENTS.md` first. Kernel and rootfs build and manipulation scripts. Bash, same conventions as `scripts/`:
`set -euo pipefail`, a header usage comment, idempotent where reasonable, and a cleanup trap on anything that
mounts or chroots.

## Files

- `fetch-test-image.sh`: kernel plus a minimal rootfs from Firecracker's public CI artifacts. Not a production
  image (no CA certificates); for a quick sanity check.
- `inject-agent.sh`: bakes the guest agent into any systemd rootfs (copy to `/usr/local/bin/`, install and enable
  the service).
- `build-guest-kernel.sh`: rebuilds `vmlinux` with `CONFIG_FUSE_FS` (off in Firecracker's configs; guest kernels
  cannot load modules). Only needed for remote storage mounts.
- `inject-rclone.sh`: bakes `rclone` and `fusermount3` into a rootfs. `fusermount3` is needed even as root because
  rclone's FUSE backend always execs it; it depends only on libc, so copy it from any Debian/Ubuntu `/bin/fusermount3`.
- `build-universal-image.sh`: builds the production `base_rootfs` with `debootstrap` (Ubuntu, Node.js LTS, Python 3,
  common tooling, systemd) as ext4, then runs `setup-multi-agent-users.sh`.
- `setup-multi-agent-users.sh`: N sequential agent users with private homes plus a shared group and directory. Runs
  from the universal build, or standalone on any ext4 image.
- `README.md`: user-facing notes for these scripts.

## Invariants

- Every script that mounts a loop device needs a cleanup trap (`inject-agent.sh`'s `trap cleanup EXIT`); a mid-mount
  death leaves a stale loop device and a locked image on the shared dev box.
- These need real root and network access. If you cannot run them, say so; someone with the dev box must run one
  before it is done.
- `base_rootfs` is the default image for every new sandbox. A new image changes nothing until that setting points at
  it and the daemon restarts. A single sandbox can boot a registered image instead (`POST /images`, `image_id`; handled
  in the daemon's `http/routes/images.rs` and `sandkiln-vmm`'s `image.rs`). The daemon cannot verify the agent is
  baked in, so run `scripts/preflight-check.sh --root-checks --rootfs-image <path>` before registering. OCI conversion
  is not attempted.

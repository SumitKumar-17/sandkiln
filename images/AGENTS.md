# AGENTS.md — images/

Read root `AGENTS.md` first. No build system of its own — kernel/rootfs
build and manipulation scripts, bash, same conventions as `scripts/`
(`set -euo pipefail`, header usage comment, idempotent where reasonable,
cleanup trap on anything that mounts/chroots).

## What exists here

- `fetch-test-image.sh` — pulls a known-good kernel + minimal rootfs from
  Firecracker's public CI artifacts. **Not a production image** (lacks
  even CA certificates) — kept for a fast manual sanity check only.
- `inject-agent.sh` — bakes the guest agent into a rootfs (mount, copy to
  `/usr/local/bin/`, install + enable a systemd service). Works against
  any systemd rootfs.
- `build-guest-kernel.sh` — rebuilds `vmlinux` with `CONFIG_FUSE_FS`
  enabled (off in Firecracker's default/CI configs; guest kernels can't
  load modules at runtime), from Firecracker's published recommended
  config, reconciled with `make olddefconfig`. Only needed for remote
  storage mounts — most sandboxes don't need it.
- `inject-rclone.sh` — bakes `rclone` + `fusermount3` into a rootfs, same
  shape as `inject-agent.sh`. `fusermount3` is needed even running as
  root: rclone's FUSE backend always execs it, no direct `mount(2)`
  fallback. Depends only on libc — copy straight from any Debian/Ubuntu
  host's `/bin/fusermount3` (`fuse3` package).
- Other image-build/multi-agent-user-setup scripts may exist beyond
  this list — check `ls` and each script's own header comment.

## Non-obvious things

- **Every loop-device-mounting script needs a cleanup trap** — a
  mid-mount death without unmounting leaves a stale loop device and a
  locked image file on the shared dev box. Follow `inject-agent.sh`'s
  `trap cleanup EXIT` pattern.
- **These scripts need real root and network access** — can't be
  meaningfully tested sandboxed. Without that access, write the script
  correctly and say so plainly; whoever has the real dev box must
  actually run it before it's "done."
- `SANDKILN_BASE_ROOTFS` controls the **default** image every new
  sandbox boots from — building a new image doesn't change that until
  the env var points at it and the daemon restarts. A single sandbox can
  instead boot from a *registered* image without touching the default
  (see Managed images below).
- **Managed images**: `routes_images.rs` (`POST/GET /images`,
  `DELETE /images/:id`) registers an already-built ext4 rootfs under a
  name, referenced per sandbox via `image_id`
  (`kiln sandbox create --image <id>`). Copies the file into
  `SANDKILN_IMAGES_DIR` — no HTTP upload, no building/converting; the
  file must already be a real ext4 rootfs with the agent injected
  (`inject-agent.sh`). The daemon can't verify that itself (no root to
  loop-mount) — `scripts/preflight-check.sh --root-checks --rootfs-image
  <path>` confirms it before registering. Converting a Docker/OCI image
  into a bootable rootfs is a separate, larger problem this mechanism
  doesn't attempt.

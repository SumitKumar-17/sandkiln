# AGENTS.md — images/

Read the root `AGENTS.md` first for project-wide conventions. This
directory has no build system of its own — it's kernel/rootfs build and
manipulation scripts, all bash, following the same conventions as
`scripts/` (`set -euo pipefail`, clear usage comments in the header,
idempotent where reasonable, a trap for cleanup on privileged scripts
that mount/chroot).

## What exists here

- `fetch-test-image.sh` — pulls a known-good kernel + minimal rootfs from
  Firecracker's public CI artifacts. **Not a production image** — it's
  what Phase-1-era manual boot testing used, kept because it's still
  useful for a fast sanity check (it lacks even CA certificates, so
  don't mistake it for something a real sandbox should boot from).
- `inject-agent.sh` — bakes the guest agent binary into a rootfs image
  (mount, copy to `/usr/local/bin/`, install + enable a systemd service).
  Works against any rootfs with systemd, including whatever
  `build-universal-image.sh` produces (if that exists yet — check).
- Whatever image-build and multi-agent-user-setup scripts exist beyond
  this — check `ls` and each script's own header comment, this file
  isn't guaranteed to enumerate every script that's been added since it
  was written.

## Non-obvious things

- **Every script here that mounts a loop device needs a cleanup trap.**
  A script that dies mid-mount without unmounting leaves a stale loop
  device and a locked image file behind — this has real consequences on
  the shared dev box (other processes, other users). Follow the
  `trap cleanup EXIT` pattern already in `inject-agent.sh`.
- **These scripts need real root and real network access to actually
  run** (debootstrap-style builds pull packages over the network; mount
  operations need root) — they cannot be meaningfully tested in an
  isolated/sandboxed environment without both. If you're an agent
  without that access, write the script correctly and say so plainly
  rather than claiming it works; whoever has access to the real dev box
  needs to actually run it before it's "done."
- The daemon's `SANDKILN_BASE_ROOTFS` env var controls which image every
  new sandbox boots from **by default** — building a new image doesn't
  change that default until the env var points at it (and the daemon is
  restarted). A single sandbox can instead boot from a *registered*
  image without touching that default at all — see "Managed images"
  below.
- **Managed images**: `core/crates/daemon/src/routes_images.rs`
  (`POST /images`, `GET /images`, `DELETE /images/:id`) lets a caller
  register an already-built ext4 rootfs file — one built with the
  scripts here — under a name, and reference it per sandbox via
  `POST /sandboxes`'s `image_id` field (`kiln sandbox create --image
  <id>`). Registration copies the file (via `sandkiln_vmm::image::ImageStore`)
  into `SANDKILN_IMAGES_DIR`; it does not accept an HTTP upload and does
  not build or convert anything — the file handed to it must already be a
  real ext4 rootfs, produced the same way `SANDKILN_BASE_ROOTFS` is (this
  directory's scripts, run out of band, with the agent already injected
  via `inject-agent.sh`). The daemon cannot verify that last step itself
  (no root to loop-mount) — `scripts/preflight-check.sh --root-checks
  --rootfs-image <path>` is the way to confirm it before registering.
  **Converting a Docker/OCI image into a bootable rootfs is a separate,
  larger problem, not attempted by this mechanism** — see "Still open"
  below.

---
title: Custom & managed images
description: Boot sandboxes from your own registered rootfs image.
---

By default every sandbox boots from the daemon's configured base rootfs (`SANDKILN_BASE_ROOTFS`). A registered image lets you boot from something else — your own tooling baked in, a specific runtime version, whatever your workload needs — without changing the daemon's own default.

## Registering an image

`POST /images` with `{"id": "<name>", "path": "<host-path>"}` registers an already-built ext4 rootfs file, copied from `path` into the daemon's managed image directory (`SANDKILN_IMAGES_DIR`). This is **not a file upload** — `path` has to already exist on the host the daemon process itself runs on.

## Booting from it

Pass `image_id` to `POST /sandboxes` instead of leaving it unset. A nonexistent `image_id` is a clean `404`, checked before the (slow) boot ever starts.

## Guest agent verification is honest, not silent

Every response from `POST/GET /images` includes `"guest_agent_verified": false` and a `verification_hint` string — the daemon runs unprivileged and can't loop-mount a candidate image as root to confirm the guest agent is actually baked in. Run `scripts/preflight-check.sh --root-checks --rootfs-image <path>` out of band, before registering, to get that confirmation yourself. This is a deliberate choice: pretending to verify something the daemon structurally can't check would be worse than saying so plainly.

## Deleting

`DELETE /images/:id` is refused (`409`) while any live sandbox, in-flight boot, or held snapshot still references it — same pattern as drives.

## What's not done yet

Only an already-built ext4 rootfs file can be registered — there's no OCI/Docker-image conversion. Converting a container image into a bootable rootfs is a separate, larger problem this project doesn't attempt yet; see `images/README.md` in the repository and the project's Roadmap page.

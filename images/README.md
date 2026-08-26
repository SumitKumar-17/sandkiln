# images

Kernel and rootfs build scripts for sandbox base images.

- `fetch-test-image.sh` — pulls a known-good kernel + rootfs from
  Firecracker's public CI artifacts, for manual boot testing. Not a
  production image source.
- `inject-agent.sh` — bakes the guest agent binary into a rootfs image and
  enables it as a systemd service.

Still open: the actual production base image (Ubuntu + Node.js LTS +
Python + common tooling), the managed image catalog, and custom image
support — see "Base and custom images" in `ROADMAP.md`.

Built artifacts (kernels, rootfs images) are not committed — see
`.gitignore` — they're large binary blobs that get rebuilt or fetched, not
tracked in git history.

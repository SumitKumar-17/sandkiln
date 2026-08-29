# images

Kernel and rootfs build scripts for sandbox base images.

- `fetch-test-image.sh` — pulls a known-good kernel + rootfs from
  Firecracker's public CI artifacts, for manual boot testing. Not a
  production image source.
- `inject-agent.sh` — bakes the guest agent binary into a rootfs image and
  enables it as a systemd service.
- `build-universal-image.sh` — builds the production universal base rootfs
  from scratch with `debootstrap`: current Ubuntu LTS, current Node.js LTS,
  Python 3, common CLI tooling, and a working systemd init, as a single
  ext4 image. This is what `SANDKILN_BASE_ROOTFS` should point at instead
  of the CI test image — that image is missing even `ca-certificates` and
  was never meant to back real sandboxes. Needs sudo. Calls
  `setup-multi-agent-users.sh` as its last step. See "Base and custom
  images" in `ROADMAP.md`.
- `setup-multi-agent-users.sh` — bakes multi-agent isolation into a rootfs
  image: sequential `agent0`..`agentN` user accounts with private home
  directories, plus a shared group and `/srv/shared` for deliberate
  cross-agent file sharing. Runs automatically as part of
  `build-universal-image.sh`, or standalone against any already-built ext4
  rootfs image. Needs sudo. See "Multi-agent isolation" in `ROADMAP.md`.

Still open: the managed image catalog and custom image (user-provided or
OCI-converted) support — see "Base and custom images" in `ROADMAP.md`.

Built artifacts (kernels, rootfs images) are not committed — see
`.gitignore` — they're large binary blobs that get rebuilt or fetched, not
tracked in git history.

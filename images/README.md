# images

Kernel and rootfs build scripts for sandbox base images. Per-script details and conventions are in
[`AGENTS.md`](AGENTS.md); the full setup walkthrough is
[Manual host setup](https://sandkiln.mintlify.site/self-hosting/manual-setup).

| Script | Purpose |
|---|---|
| `fetch-test-image.sh` | kernel plus a small rootfs from Firecracker's CI artifacts; for manual testing, not production |
| `inject-agent.sh` | bake the guest agent into a rootfs and enable its systemd service |
| `build-universal-image.sh` | production rootfs with `debootstrap` (Ubuntu, Node.js, Python 3, tooling); needs sudo |
| `setup-multi-agent-users.sh` | per-agent users with private homes and a shared `/srv/shared`; run by the universal build |
| `build-guest-kernel.sh` | kernel with `CONFIG_FUSE_FS`, only needed for remote storage mounts |
| `inject-rclone.sh` | bake `rclone` and `fusermount3` into a rootfs, for remote storage mounts |

Register a built image with the daemon through `POST /images` or `kiln image create <id> <path>`, then pick it per sandbox
with `image_id` or `kiln sandbox create --image <id>`. The file must already be an ext4 rootfs with the agent injected;
OCI-to-rootfs conversion is not implemented (see `ROADMAP.md`).

Built artifacts are not committed (see `.gitignore`).

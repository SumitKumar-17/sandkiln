# images

Kernel and rootfs build scripts for sandbox base images.

- `fetch-test-image.sh` — pulls a known-good kernel + rootfs from
  Firecracker's public CI artifacts, for manual boot testing. Not a
  production image source.
- `inject-agent.sh` — bakes the guest agent binary into a rootfs image and
  enables it as a systemd service. `--init` instead makes the agent the
  guest's PID 1 (see "Init-mode image" below).
- `build-guest-kernel.sh` — rebuilds `vmlinux` with `CONFIG_FUSE_FS`
  enabled (off by default; guest kernels can't load modules at runtime).
  Only needed for remote storage mounts — most sandboxes don't need it.
- `inject-rclone.sh` — bakes `rclone` + `fusermount3` into a rootfs, same
  shape as `inject-agent.sh`. Needed even running as root: rclone's FUSE
  backend always execs `fusermount3`, no direct `mount(2)` fallback.
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

**Done: registering multiple named images and picking one per sandbox.**
An already-built ext4 rootfs (produced with the scripts above) can be
registered with the daemon under a name (`POST /images`, `kiln image
create <id> <path>`) and referenced by a specific `POST /sandboxes` call
(`image_id`, `kiln sandbox create --image <id>`) instead of the
daemon-wide `SANDKILN_BASE_ROOTFS` default — see
`docs-site/getting-started/self-hosting.mdx` and `docs-site/self-hosting/manual-setup.mdx` ("Custom images") and `core/crates/daemon/src/http/routes/images.rs`.
This does not accept a file upload or build anything; `path` must already
be a real ext4 rootfs on the daemon's own host.

Still open: **OCI conversion** — turning a Docker/OCI image into a
bootable ext4 rootfs (layers, entrypoint, etc.) — see "Base and custom
images" in `ROADMAP.md`. That remains a separate, larger problem than
registering an image that's already built.

## Init-mode image (opt-in, faster cold start)

`sudo images/inject-agent.sh --init <agent-binary> <copy-of-rootfs.ext4>`
points `/sbin/init` at the agent, so the kernel starts it as PID 1 and
systemd never runs. The agent mounts the pseudo filesystems, sets the
hostname, brings loopback up and reaps orphans itself (`core/crates/guest-agent/src/init/`).
Always run it on a **copy**: the default images are untouched, and the copy is
registered and selected per sandbox like any custom image:

```
cp --reflink=auto base.ext4 base-init.ext4        # keep it on the reflink filesystem
sudo images/inject-agent.sh --init core/target/x86_64-unknown-linux-musl/release/sandkiln-agent base-init.ext4
kiln image create init-agent base-init.ext4        # then: kiln sandbox create --image init-agent
```

Measured on the dev box, cold `create_to_first_command` median 230ms with the
systemd image against 147ms with this one (see `docs/benchmarks.md`). One
operational catch: the daemon copies a registered image into `images_dir`;
if that directory is not on the filesystem that supports reflink, every
create pays a full 300MB copy and the gain disappears (create call 22ms ->
98ms measured). Point `images_dir` at the reflink volume. Re-running
`inject-agent.sh` without `--init` on the same file restores systemd.

What an init-mode guest does not have: systemd units (services baked into the
image never start), journald, `systemctl`/`poweroff` talking to a manager
(send `SIGTERM` to PID 1 or `reboot -f`), and the ssh service. Not yet
verified: remote storage mounts (needs the FUSE kernel and an object store).

Built artifacts (kernels, rootfs images) are not committed — see
`.gitignore` — they're large binary blobs that get rebuilt or fetched, not
tracked in git history.

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
hostname from `/etc/hostname`, brings loopback up, takes `LANG`/`LC_*` from the
image's own locale files and reaps orphans itself
(`core/crates/guest-agent/src/init/`). Always run it on a **copy**: the default
images are untouched, and the copy is registered and selected per sandbox like
any custom image:

```
cp --reflink=auto base.ext4 base-init.ext4        # keep it on the reflink filesystem
sudo images/inject-agent.sh --init core/target/x86_64-unknown-linux-musl/release/sandkiln-agent base-init.ext4
kiln image create init-agent base-init.ext4        # then: kiln sandbox create --image init-agent
```

From scratch, `build-universal-image.sh` does both steps:
`sudo images/build-universal-image.sh --agent <agent-binary> --init out.ext4 [size-gb] [codename] [agents]`
(without `--init` the agent is injected as the systemd service; without
`--agent` nothing is injected). `scripts/dev.sh inject-agent --init [rootfs]`
builds the agent and injects it in init mode. Re-running `inject-agent.sh`
without `--init` on the same file restores systemd.

Measured on the dev box, cold `create_to_first_command` median 232ms with the
systemd image against 147ms with this one, n=60 each (see
`docs/benchmarks.md`). One operational catch: the daemon copies a registered
image into `images_dir`; if that directory is not on the filesystem that
supports reflink, every create pays a full 300MB copy and the gain disappears
(create call 22ms -> 98ms measured). Point `images_dir` at the reflink volume.

### What init mode replaces and what it drops

Replaced by the agent itself: the pseudo filesystem mounts (`/proc /sys
/dev/pts /dev/shm /run /tmp`, cgroup2), `/dev/fd` and `stdin/out/err` links,
the hostname, `lo`, `PATH` and the locale, the open-files limit, a clean
sync-and-reset on `SIGTERM`/`SIGINT`, and restarting the agent if it dies
(capped backoff). The network interface was already configured by the kernel
`ip=` argument, not by a guest unit, so `fcnet.service` in the CI image and
`/etc/hosts` (a plain file in every image we build) need no replacement.

Dropped, because nothing runs them: every systemd unit baked into the image.
The CI image enables `ssh.service`, `getty@tty1`, `fcnet.service`,
`systemd-pstore`, `tmp.mount` and `var-lib-systemd.mount`. The universal image
(read from a real build) enables `cron`, `rsyslog`, `systemd-resolved`,
`systemd-timesyncd`, `networkd-dispatcher`, the console and keyboard setup
units, `dmesg`, the Ubuntu Pro units and the usual timers (fstrim, logrotate,
dpkg-db-backup, motd-news; the apt timers are masked by the build). Nothing in
sandkiln talks to a guest over ssh or the console, and nothing in the API, the
SDKs, the CLI or the integration suite needs any of them, so they are not
replicated. Two have a visible effect on a workload: **cron jobs do not run**
(start `cron` from your own command if you need it), and **the clock is not
disciplined by timesyncd** (the guest clock is correct at boot and after a
resume, which sets it from the host; it is not corrected afterwards).
Name resolution does not depend on a unit: `build-universal-image.sh` writes a
plain `/etc/resolv.conf` (the bridge gateway, `GUEST_NAMESERVER`) so it works in
both modes. An image that relies on its own units (a database, an sshd) must
start them from the command that uses them or stay on the systemd image. Also
absent: journald, `systemctl`/`poweroff` talking to a manager (send `SIGTERM`
to PID 1, or `reboot -f`). A guest that shuts itself down leaves its sandbox
listed with a dead VM until it is deleted (finding 33; the same with systemd).

Verified on the dev box with a FUSE kernel, `rclone` in the image and a local
S3-compatible endpoint: the remote storage mounts topic passes on an init-mode
image (`inject-rclone.sh` works on an init image too; mounting needs the
image to have room for the 85MB `rclone` binary, the 300MB CI image does not).

Built artifacts (kernels, rootfs images) are not committed — see
`.gitignore` — they're large binary blobs that get rebuilt or fetched, not
tracked in git history.

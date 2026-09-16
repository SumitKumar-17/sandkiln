# sandkiln remote storage mount

A minimal reference example of remote storage mounts: mount an
S3-compatible bucket into a sandbox's own filesystem, write a file into
it and read it back through the mount, then unmount — so code running
inside the sandbox reads and writes object storage with ordinary
filesystem calls, no SDK or bucket client of its own.

## What it does

1. Creates a sandbox with `Sandbox.create()`.
2. `sandbox.mount({ bucket, endpoint, accessKey, secretKey, mountPath })`
   mounts your bucket at `/mnt/bucket` inside the guest. The daemon
   writes a `0600` rclone config into the guest and starts `rclone
   mount` there — credentials never appear on a command line, and the
   daemon keeps no copy.
3. `sandbox.listMounts()` to list what's mounted.
4. Writes a file at `/mnt/bucket/sandkiln-example-<timestamp>.txt` with
   `sandbox.writeFile()`, lists the directory with `sandbox.runCommand()`,
   and reads it back with `sandbox.readFile()` — all plain filesystem
   operations, which land in the bucket as a real object.
5. `sandbox.unmount(mount.id)` to unmount, then confirms with
   `mountpoint -q` that the FUSE mount is genuinely gone from the guest.
6. Stops the sandbox with `sandbox.stop({ keep: false })` — the object
   written in step 4 stays in the bucket.

See `index.js` — it's the whole program.

## Requirements

A running `sandkilnd` daemon — see
[`SELF_HOSTING.md`](../../SELF_HOSTING.md) at the repo root. There is no
hosted service. Mounts additionally need the two pieces of setup in that
guide's **"Remote storage mounts (optional)"** section, neither of which
a default install gives you:

- a guest kernel built with `CONFIG_FUSE_FS`
  (`images/build-guest-kernel.sh`) — stock Firecracker kernels don't
  enable FUSE and guest kernels can't load modules at runtime, so
  `/dev/fuse` otherwise doesn't exist at all;
- `rclone` and `fusermount3` injected into the rootfs
  (`scripts/dev.sh inject-rclone`).

You also need an S3-compatible endpoint of your own, reachable **from the
guest's network**, not just from your laptop — the mount client runs
inside the microVM. Any self-hosted or provider-agnostic S3-compatible
object store works; nothing about this is tied to a particular one.

### Standing up a local test target

The easiest option needs nothing new installed: `rclone` — already a
dependency of this feature — can serve a local directory as an
S3-compatible endpoint. This is exactly how the feature itself was
verified:

```
mkdir -p /tmp/s3data/my-bucket
rclone serve s3 /tmp/s3data \
  --addr <address-the-guest-can-reach>:9000 \
  --auth-key "testkey,testsecret"
```

Bind it to an address the sandbox can actually route to — the host's
sandbox bridge address (`SANDKILN_BRIDGE_GATEWAY`, `172.16.0.1` by
default), not `127.0.0.1`, which inside the guest means the guest itself. Objects show up as ordinary
files under `/tmp/s3data/my-bucket`, so you can confirm from the host
that a write really landed.

## Run it

```
cd examples/remote-storage-mount
npm install
S3_ENDPOINT=http://172.16.0.1:9000 \
S3_ACCESS_KEY=testkey \
S3_SECRET_KEY=testsecret \
S3_BUCKET=my-bucket \
node index.js
```

## Configuration

- `S3_ENDPOINT` — full URL of the S3-compatible endpoint, as reachable
  from inside the sandbox. Required; never defaulted, so a mount's target
  is always explicit.
- `S3_ACCESS_KEY` / `S3_SECRET_KEY` — credentials for it. Required.
- `S3_BUCKET` — the bucket to mount. Required.
- `SANDKILN_DAEMON_URL` — base URL of the daemon. Defaults to
  `http://127.0.0.1:7777`.
- `SANDKILN_AUTH_TOKEN` — auth token, only needed if the daemon was
  started with one.

## Known limitations

- One mount per path: mounting a second bucket at an already-mounted
  `mount_path` is a `409`.
- Mounts are not re-applied on resume/fork/restore, and don't need to be:
  the FUSE client is a live process inside the guest, so Firecracker's
  snapshot captures it along with the rest of guest memory and it keeps
  working.
- Performance is object-storage performance over FUSE, not local disk —
  use a drive (`SELF_HOSTING.md`'s drives section) for anything
  latency-sensitive.

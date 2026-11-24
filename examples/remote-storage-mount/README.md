# sandkiln remote storage mount

Mount an S3-compatible bucket into a sandbox, write and read a file through the mount, then unmount, so code in the sandbox uses object storage with ordinary filesystem calls.

## What it does

1. Creates a sandbox and calls `mount({ bucket, endpoint, accessKey, secretKey, mountPath })` to mount at `/mnt/bucket`. The daemon writes a `0600` rclone config in the guest and starts `rclone mount`; credentials never appear on a command line and the daemon keeps no copy.
2. Lists mounts with `listMounts()`.
3. Writes `/mnt/bucket/sandkiln-example-<timestamp>.txt` with `writeFile()`, lists the directory and reads the file back. It lands in the bucket as an object.
4. Calls `unmount(mount.id)` and checks with `mountpoint -q` that it is gone.
5. Calls `stop({ keep: false })`. The object stays in the bucket.

The whole program is `index.js`.

## Requirements

A running `sandkilnd` daemon ([`SELF_HOSTING.md`](../../SELF_HOSTING.md)). There is no hosted service. Mounts also need a guest kernel with `CONFIG_FUSE_FS` (`images/build-guest-kernel.sh`) and `rclone` plus `fusermount3` in the rootfs (`scripts/dev.sh inject-rclone`); see [Manual host setup](https://sandkiln.mintlify.site/self-hosting/manual-setup#remote-storage-mounts-optional).

You also need an S3-compatible endpoint reachable from the guest's network, not just your laptop. For a local test target, `rclone` can serve a directory:

```
mkdir -p /tmp/s3data/my-bucket
rclone serve s3 /tmp/s3data --addr 172.16.0.1:9000 --auth-key "testkey,testsecret"
```

Bind to the sandbox bridge address (`bridge_gateway`, `172.16.0.1` by default), not `127.0.0.1`, which inside the guest is the guest itself.

## Run it

```
cd examples/remote-storage-mount
npm install
S3_ENDPOINT=http://172.16.0.1:9000 S3_ACCESS_KEY=testkey \
S3_SECRET_KEY=testsecret S3_BUCKET=my-bucket node index.js
```

## Configuration

- `S3_ENDPOINT`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`, `S3_BUCKET` (all required): the endpoint URL as reachable from inside the sandbox, its credentials and the bucket. The endpoint is never defaulted.
- `SANDKILN_DAEMON_URL`: daemon base URL (default `http://127.0.0.1:7777`).
- `SANDKILN_AUTH_TOKEN`: only needed if the daemon has a token.

## Notes

- One mount per path; a second mount at the same `mount_path` gets `409`.
- Mounts survive resume, fork and restore because the FUSE client is a live guest process captured in the snapshot.
- Performance is object storage over FUSE; use a drive for anything latency-sensitive.

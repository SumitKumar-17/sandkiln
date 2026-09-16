---
title: Remote storage mounts
description: Mount an S3-compatible bucket into a sandbox and read and write it as an ordinary directory.
---

A mount attaches an S3-compatible bucket to a path inside a sandbox, so code running there reads and writes objects through ordinary filesystem calls instead of an object-store client. It's a FUSE filesystem (`rclone mount`) running inside the guest — Firecracker has no `virtio-fs` or `9p` support at all, so a host-side directory passthrough was never an option.

## Two prerequisites before this works at all

**A guest kernel built with `CONFIG_FUSE_FS`.** Firecracker's own default and CI kernel configurations don't enable it, and guest kernels can't load modules at runtime, so `/dev/fuse` simply doesn't exist in a stock setup. Build one with `images/build-guest-kernel.sh [version] [out-path]` (defaults to 5.10.223) and point the daemon at it with `SANDKILN_KERNEL_PATH`.

**`rclone` and `fusermount3` baked into the rootfs.** Both are injected as binaries — `images/inject-rclone.sh`, or `scripts/dev.sh inject-rclone <static-rclone-binary> [rootfs]` — the same way the guest agent itself is, rather than installed with a package manager. `fusermount3` is a hard requirement, not a convenience: rclone's Linux FUSE backend always execs it to perform the actual mount, even running as root, and has no direct-mount code path.

There is no preflight capability check. The routes are always registered, so a missing kernel feature or missing binary shows up only when you try to mount, as a `400` whose message is rclone's own `rclone mount failed (exit N): ...` output. See [Self-hosting quickstart](../../getting-started/self-hosting/) for the setup steps in context.

## Mounting, listing, unmounting

`Sandbox.mount()`/`.listMounts()`/`.unmount()` (JS/TS), `mount()`/
`list_mounts()`/`unmount()` (Python), and `kiln sandbox mount|mounts|
unmount` (CLI) all wrap the same three routes below — reach for those
first; the raw HTTP shown here is what they call underneath, and is the
only surface if you're integrating from a language without a published
SDK.

```bash
TOKEN=...
BASE=http://127.0.0.1:7777

curl -s -X POST "$BASE/sandboxes/build-worker/mounts" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"bucket":"datasets","endpoint":"https://s3.internal:9000",
       "access_key":"...","secret_key":"...",
       "mount_path":"/mnt/datasets","read_only":false}'
# {"id":"...","bucket":"datasets","endpoint":"https://s3.internal:9000","mount_path":"/mnt/datasets","read_only":false}

curl -s "$BASE/sandboxes/build-worker/mounts" -H "Authorization: Bearer $TOKEN"
# {"mounts":[{"id":"...","bucket":"datasets",...}]}

curl -s -X DELETE "$BASE/sandboxes/build-worker/mounts/<mount-id>" \
  -H "Authorization: Bearer $TOKEN"
# 204 No Content
```

`endpoint` is required and never defaulted, so a mount's target is always explicit. `mount_path` is created (`mkdir -p`) if it isn't already there, and `read_only: true` passes rclone's read-only flag — the default is read-write. Mounting a second bucket at a `mount_path` already in use in the same sandbox is a `409` naming the existing bucket and the `DELETE` URL that clears it. Full field and error-code details are in the [Daemon HTTP API](../../reference/http-api/#remote-storage-mounts) reference.

The daemon verifies the result rather than trusting rclone's exit code: after a successful mount it runs `mountpoint -q <path>` inside the guest, and if that disagrees it unmounts, removes the config, and returns a `500`. Unmounting is best-effort past the bookkeeping — the mount is dropped from daemon state whether or not the guest-side `umount` succeeded, and a stuck mount is logged as a warning rather than failing the request.

## Credentials

Credentials go into the guest as a small single-remote rclone config file, written with the same `write-file` and `chmod` guest operations the filesystem routes expose, locked to `0600` before rclone ever runs. They're never passed as a command-line argument, so nothing inside the guest can read them out of `ps`. The daemon doesn't log them, doesn't persist them on the host, and never echoes them back — a mount response carries only `id`, `bucket`, `endpoint`, `mount_path`, and `read_only`.

## What's not done yet

Mounts are not re-applied on resume, fork, or restore. A mount is a live guest-side FUSE process, captured along with everything else in the snapshot of guest memory, so a restored sandbox's mount keeps working with no daemon involvement — but nothing re-establishes one that didn't survive. The recorded metadata is carried on snapshots purely so `GET /sandboxes/:id/mounts` can answer without a round-trip into the guest; it isn't surfaced in sandbox or snapshot list responses.

There's no holder-tracking or exclusivity the way [Drives](../drives/) have it — two sandboxes may mount the same bucket at the same time. The only conflict rule is a duplicate `mount_path` within a single sandbox. `mount_path` itself gets no validation beyond being non-empty.

S3-compatible object stores only. The generated rclone config is a fixed `s3`-type remote with a generic provider and an explicit endpoint, authenticating with a static access key and secret key — no other rclone backends, no IAM-role-style auth, no session tokens.

Verified end to end against a local S3-compatible test fixture (`rclone serve s3`), not against any hosted object store. The opt-in integration test at `scripts/integration-tests/22-mounts.sh` runs only when `SANDKILN_MOUNTS_TEST_ENDPOINT` is set, and skips otherwise.

---
title: "Internals: FUSE and rclone"
description: What FUSE and rclone actually are, and how a mount is built from ordinary guest operations with no new wire protocol.
---

For the request/response shape and the SDK/CLI surface, see [Remote storage mounts](../../concepts/remote-storage/). This page is about the two underlying technologies doing the actual work inside the guest.

## What it is

**FUSE** (Filesystem in Userspace) is a Linux kernel interface that lets an ordinary, unprivileged process implement a filesystem. Normally a filesystem driver lives in kernel space (ext4, xfs, and so on); FUSE instead exposes a small kernel module (`/dev/fuse`) that forwards every filesystem operation, an `open`, a `read`, a `readdir`, to a userspace program, which answers however it wants. The kernel doesn't care what's on the other end: a network share, a compressed archive, or in this case an object-storage bucket.

**rclone** is the userspace program on the other end here. It's a general-purpose tool for moving files to and from dozens of storage backends, and one of its subcommands, `rclone mount`, uses FUSE to present a bucket as a real directory tree: `ls`, `cat`, and `>` all work as if the objects were local files, translated into S3 API calls behind the scenes.

## Why sandkiln uses it here

Firecracker has no `virtio-fs` or `9p` support at all. That's an upstream constraint, not a choice this project made: there is no host-to-guest shared-directory mechanism to reach for, so a host-side bucket mount could never be exposed into the guest the way it might be in a container runtime. The only place a FUSE client can run is *inside* the guest, against the guest's own kernel.

Given that, rclone was the pragmatic choice over writing a custom FUSE client: it already speaks S3-compatible APIs correctly, already has a mature FUSE backend, and ships as a single static binary that can be injected into a rootfs the same way the guest agent itself is (see `images/inject-rclone.sh`).

## Key terms

- **FUSE**: the kernel/userspace interface described above. Requires `/dev/fuse` to exist, which requires the guest kernel to be built with `CONFIG_FUSE_FS` (`images/build-guest-kernel.sh`) -- Firecracker's own default and CI kernel configs don't enable it.
- **`fusermount3`**: a small setuid helper that lets a non-root user mount a FUSE filesystem without extra capabilities. rclone's Linux FUSE backend always execs it internally to perform the actual `mount(2)` call, even when the calling process is already root -- there's no direct-mount code path that skips it. sandkiln's guest agent runs as root (see `images/inject-agent.sh`'s systemd unit), so this isn't for privilege elevation here, just because rclone always goes through it regardless of caller.
- **rclone remote**: rclone's own term for one configured storage backend (an endpoint plus credentials). sandkiln writes a minimal single-remote config file per mount, always named `sandkiln`, always `type = s3, provider = Other` with an explicit `endpoint` -- generic enough for any S3-compatible store, not tied to one provider.

## How it works in sandkiln

`core/crates/daemon/src/routes_mounts.rs` needed no new entry in the host-guest wire protocol at all. A mount is built entirely from operations `sandkiln-protocol` already has:

1. `WriteFile` writes the rclone config (the endpoint and credentials) to a path under `/root/`.
2. `Chmod` locks it to `0600` before rclone ever runs, so nothing else on the guest (`ps aux`, another process reading `/proc`) can see the credentials -- they're in a file, never a command-line argument.
3. `Exec` runs `rclone mount <remote>:<bucket> <mount_path> --config <path> --daemon --daemon-wait=30s`, backgrounding rclone's own process so the `Exec` call itself returns once the mount is confirmed ready rather than blocking on a long-running foreground process.
4. A second `Exec` runs `mountpoint -q <path>` to verify the result independently of rclone's own exit code -- if that disagrees, the daemon unmounts, removes the config, and reports a `500` rather than trusting a "success" that isn't real.

Unmounting is the same shape in reverse: `Exec umount`, then `Exec rm -f` on the config file. Nothing about a mount is re-applied on resume, fork, or a restored checkpoint -- rclone is a live guest-side process, and Firecracker's snapshot mechanism already captures a running process's full state along with everything else in guest memory, so a resumed sandbox's mount just keeps working with zero daemon involvement.

## See it in action

This dev box's test rootfs doesn't have rclone injected (no real S3-compatible endpoint is configured here either -- see `examples/remote-storage-mount/README.md`'s own caveat about that), so the honest example is the clean-failure path, which is itself a real, useful thing to see: a missing prerequisite reports as an ordinary `400` with rclone's own error text, not a crash or an opaque `500`.

```
$ curl -si -X POST http://127.0.0.1:7777/sandboxes/<id>/mounts \
    -H 'content-type: application/json' \
    -d '{"bucket":"demo-bucket","endpoint":"http://127.0.0.1:1","access_key":"demo","secret_key":"demo","mount_path":"/mnt/data"}'

HTTP/1.1 400 Bad Request
content-type: application/json
x-request-id: b63384ec-b00c-493e-921d-bb363ed08c8f
content-length: 50
date: Wed, 16 Sep 2026 05:25:10 GMT

{"error":"No such file or directory (os error 2)"}
```

That's `rclone`'s own binary not existing on this particular test image (see `images/inject-rclone.sh`), surfaced as-is rather than swallowed. A rootfs with rclone and `fusermount3` actually injected, mounted against a real reachable S3-compatible endpoint, returns the `200` shape documented in [Remote storage mounts](../../concepts/remote-storage/#mounting-listing-unmounting).

---
title: "Images: the mechanism"
description: "What a rootfs image actually is at the filesystem level, and the honest limit on what sandkiln can verify about one."
---

[Custom & managed images](../../concepts/images/) covers how to register and boot from one. This page is what an image actually is on disk, and why the daemon can't fully verify one it didn't build itself.

## What it is

A "rootfs image" here is a single ext4 filesystem image file: one file that, mounted, looks like a normal Linux root filesystem (`/bin`, `/etc`, `/usr`, and so on), the same shape the daemon's own default `SANDKILN_BASE_ROOTFS` already is. Firecracker attaches it to a VM as the boot drive the same way it attaches any other `virtio-blk` device (see [Drives: the mechanism](./drives/)). There's no separate "image" concept at the hypervisor level, just a block device the kernel happens to boot from because Firecracker's boot-source configuration points at it.

**ext4 superblock**: every ext2/3/4 filesystem has a fixed-format header (the superblock) at a fixed byte offset from the start of the device, containing a magic number identifying it as ext-family. Checking for that magic number is a cheap, privilege-free way to confirm "this file at least looks like an ext4 filesystem" without needing to mount it.

## Why sandkiln uses it here

Every sandbox used to be stuck with one daemon-wide default rootfs. Registering an image lets a caller boot from something else instead, their own tooling baked in, a specific runtime version, without touching that daemon-wide default for everyone else. Registration deliberately isn't an HTTP upload: accepting an arbitrary multi-gigabyte file over HTTP is a distinct, larger problem this project doesn't attempt. Instead, `path` has to already exist on the same host the daemon process runs on, and registration *copies* it into a managed directory, mirroring `DriveStore`'s own "one file per resource, filesystem is the source of truth" pattern, so a later edit, move, or deletion of the original source file can never corrupt or orphan a sandbox already booted from the registered copy.

## Key terms

| Term | Meaning here |
| --- | --- |
| **Registration** | Copying an already-built ext4 file into the daemon's managed image directory under a caller-given id, not building or uploading one. |
| `guest_agent_verified` | Always `false` in every response, on purpose. See below. |
| **Loop-mount** | Mounting a filesystem image file as if it were a block device, to inspect its actual contents. This is what would be needed to confirm the guest agent binary is really inside an image, and it needs real root. |

## How it works in sandkiln

`sandkiln_vmm::image::ImageStore` (`core/crates/vmm/src/image.rs`) is structurally identical to `DriveStore`: one file per image, named `<id>.ext4`, a directory the daemon owns. `register(id, source)` runs a specific sequence, in order, each step a real check rather than an assumption:

1. Validate `source`'s metadata exists and is a regular file.
2. Check the ext4 superblock magic (`looks_like_ext4`) at the fixed offset. This catches "this isn't even an ext4 image" mistakes, nothing more.
3. Copy it into the managed directory with `cp --reflink=auto`, the same instant copy-on-write clone (on a filesystem that supports it) the per-sandbox rootfs clone and drive creation both use.
4. Compare the source and destination file sizes afterward, rather than trusting `cp`'s own exit code alone. A near-full disk can make a copy come up short while `cp` still exits `0`, a failure mode this project has actually hit before (a truncated binary during a disk-full `npm install`, unrelated to images but the same root cause: never trust a copy's exit status alone when disk space is in question).

**What this module cannot check, stated plainly rather than silently skipped**: whether the guest agent binary is actually baked into the image and will actually respond over vsock once booted. That confirmation needs loop-mounting the file and inspecting its contents as root, and the daemon runs unprivileged by design (ambient `CAP_NET_ADMIN` only, see the project's Security hardening notes). Rather than pretend to verify something it structurally can't, every `POST`/`GET /images` response carries `"guest_agent_verified": false"` and a `verification_hint` string pointing at `scripts/preflight-check.sh --root-checks --rootfs-image <path>`, a real, separate, out-of-band check a caller can run with actual root before trusting a candidate image. A sandbox booted from an unverified image that turns out to be missing the guest agent will boot successfully and then never respond to `exec`, a real, silent failure mode this hint exists specifically to warn about in advance.

## See it in action

The verification-hint text, exactly as a caller sees it, plus what happens booting from a valid vs. nonexistent image:

```
$ curl -s http://127.0.0.1:7777/images
{"images":[{"id":"integration-test-image-174204","size_mib":300,"created_at_unix":1788205903,
  "in_use_by":null,"guest_agent_verified":false,
  "verification_hint":"the daemon cannot verify the guest agent is baked into this image (that needs loop-mounting it as root, which this unprivileged daemon does not have) — run 'scripts/preflight-check.sh --root-checks --rootfs-image <path>' against the source file before relying on it, or a sandbox booted from it may boot but never respond to exec"}, ...]}

$ curl -s -X POST http://127.0.0.1:7777/sandboxes -H 'content-type: application/json' \
    -d '{"image_id":"integration-test-image-174204"}'
{"id":"ecc699a4-fa84-4bd2-9346-5e5c6b320e9e"}

$ curl -si -X POST http://127.0.0.1:7777/sandboxes -H 'content-type: application/json' \
    -d '{"image_id":"does-not-exist"}'
HTTP/1.1 404 Not Found
content-type: application/json
x-request-id: 065657aa-2cd0-4d79-b838-373ccfb3de58
content-length: 43

{"error":"image not found: does-not-exist"}
```

The nonexistent-image case is checked and rejected before the daemon starts the (slow) boot sequence at all. A caller never waits for a boot that was always going to fail.

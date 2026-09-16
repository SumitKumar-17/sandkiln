---
title: "Drives: the mechanism"
description: "What a Firecracker drive actually is on disk, and how sandkiln tracks who's allowed to hold one at once."
---

[Drives](../../concepts/drives/) covers how to create and attach one. This page is what a drive actually is underneath, and how the daemon decides whether a second attachment is safe.

## What it is

A Firecracker "drive" is a `virtio-block` device: an emulated block device the guest kernel sees and can `mount` exactly like a real disk, backed by an ordinary file on the host. There's no virtualization magic beyond that. The guest issues block reads and writes, Firecracker translates them into reads and writes against the backing file, and whatever filesystem the guest formats it with (ext4, in every case here) is the guest's own concern. A drive is durable and reattachable specifically because "the backing file" is just a file sandkiln keeps around after the sandbox that used it stops. Nothing about it is tied to any one VM process.

## Why sandkiln uses it here

The per-sandbox rootfs a VM boots from is deliberately ephemeral, cloned fresh at boot time and torn down when the sandbox is destroyed, the same way a container's own filesystem layer normally is. A drive exists for the opposite case: state a caller explicitly wants to survive past one sandbox's lifetime and reattach to a different one later, such as a persistent cache, a shared dataset, or a working directory that should outlive whichever sandbox is currently using it.

## Key terms

| Term | Meaning here |
| --- | --- |
| `virtio-blk` | The paravirtualized block-device interface Firecracker exposes to the guest, the same device class the per-sandbox rootfs itself uses, just a second, independently attachable one. |
| **Holder** | Whatever currently has a drive attached: `sandbox <id>` or `snapshot <id>` (a held snapshot keeps its drive attachment recorded too, since resuming it needs to reattach the same drive). |
| **Exclusive** vs. **shared** attachment | A read-write attachment needs to be the *only* attachment; any number of read-only attachments can coexist with each other, but not with a read-write one. |
| `drive_id` | The identifier Firecracker itself uses per-VM to distinguish attached block devices, distinct from sandkiln's own drive id, which is a UUID and gets its hyphens replaced before being handed to Firecracker (Firecracker's own `drive_id` only allows alphanumerics and underscores). |

## How it works in sandkiln

`sandkiln_vmm::drive::DriveStore` (`core/crates/vmm/src/drive.rs`) manages a directory of drive image files, one per drive, named `<id>.ext4`. Creating one is genuinely simple: allocate a sparse file of the requested size (`file.set_len`, so actual disk usage grows with what the guest writes rather than being reserved up front, the same "let the filesystem do the work" approach the per-sandbox rootfs clone uses), then run `mkfs.ext4` against it. `DriveStore` itself has no idea which sandbox, if any, currently has a drive attached. That tracking lives entirely on the daemon side, in `sandkiln-daemon::state`, because only the daemon has the concept of a running sandbox at all.

The actual exclusivity rule is one small, pure function, `can_attach_read_only` (`core/crates/daemon/src/state.rs`):

```rust
pub fn can_attach_read_only(existing: &[bool], requesting_read_only: bool) -> bool {
    existing.is_empty() || (requesting_read_only && existing.iter().all(|ro| *ro))
}
```

Read as English: a new attachment is allowed if nothing holds the drive yet, or if the new attachment is read-only *and* every existing holder is also read-only. A single read-write holder, existing or newly requested, always forces exclusivity. `drive_holders(drive_id)` walks both the live sandbox map and the held-snapshot map to build the list this function checks against, and `describe_drive_holders` renders it into the human-readable `409` message a caller actually sees (`"sandbox a (read-only), sandbox b"`).

## See it in action

Create a drive, attach it read-write, then try attaching it read-write again while it's still held:

```
$ curl -s -X POST http://127.0.0.1:7777/drives -H 'content-type: application/json' -d '{"size_mib":32}'
{"id":"037aecc5-b10a-475c-8dba-27d82bb11402","size_mib":32,"created_at_unix":1789536245,"attached_to":[]}

$ curl -s -X POST http://127.0.0.1:7777/sandboxes -H 'content-type: application/json' \
    -d '{"drives":[{"id":"037aecc5-b10a-475c-8dba-27d82bb11402","read_only":false}]}'
{"id":"12b9dd16-5787-44fa-aceb-5c9bb404b854"}

$ curl -si -X POST http://127.0.0.1:7777/sandboxes -H 'content-type: application/json' \
    -d '{"drives":[{"id":"037aecc5-b10a-475c-8dba-27d82bb11402","read_only":false}]}'
HTTP/1.1 409 Conflict
content-type: application/json
x-request-id: 1603f6a4-3693-46d4-8b14-3b296fbbccfe
content-length: 226

{"error":"drive 037aecc5-b10a-475c-8dba-27d82bb11402 is already attached to sandbox 12b9dd16-5787-44fa-aceb-5c9bb404b854 — a read-write attachment needs exclusive access; only simultaneous read-only attachments are allowed"}

$ curl -s http://127.0.0.1:7777/drives
{"drives":[...,{"id":"037aecc5-b10a-475c-8dba-27d82bb11402","size_mib":32,"created_at_unix":1789536245,
  "attached_to":[{"holder":"sandbox 12b9dd16-5787-44fa-aceb-5c9bb404b854","read_only":false}]}]}
```

`attached_to` is exactly `can_attach_read_only`'s input, rendered back out. The same data the exclusivity check reasons over is what `GET /drives` shows you.

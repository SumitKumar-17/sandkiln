---
title: "Snapshot, resume, and fork: the mechanism"
description: "What a Firecracker snapshot actually contains, why resume and fork are so different underneath, and the rootfs-sharing bug this project shipped and then fixed."
---

[Snapshots, resume, and fork](../../concepts/snapshots/) covers how to call this from the API. This page is what's actually happening underneath, which is worth reading before debugging a resumed sandbox that boots into the wrong filesystem or can't reach its own network.

## What it is

A Firecracker "snapshot" is not a copy of a virtual disk, the way a cloud provider's VM snapshot usually is. It's two files: a **memory file** (the guest's RAM, byte for byte, at the instant it was paused) and a **device-state file** (everything about the emulated hardware, serialized to disk: the vCPU registers, the virtio-block/virtio-net device configuration, the vsock setup). Resuming means starting a brand-new Firecracker process and telling it to load those two files instead of running a kernel boot sequence. The guest picks up running from the exact instruction it was paused on, with all of its RAM already in place. No init scripts, no service startup, no dependency loading, because none of that needs to happen again.

Two consequences fall directly out of that shape, and both matter in this codebase specifically:

- **The device-state file records the rootfs drive by its host file path, not its contents.** Firecracker never copies the backing file into the snapshot. Resume reopens whatever file is sitting at that recorded path at the moment you resume. If it's been deleted, moved, or (the bug below) silently mutated by something else, the guest boots believing one filesystem is attached while a different one is actually there.
- **A VM's network identity is guest-OS state, frozen into the memory image.** The guest's IP and MAC address are set via kernel boot arguments at the *original* boot, and the host has no way to reach into a running guest's kernel and rewrite them afterward. A resumed guest keeps addressing packets to whatever it believes its own identity is, so the *host* side (the tap device, the lease) has to stay the exact one that was live at snapshot time, not a fresh one.

## Why sandkiln uses it here

This is the standard technique for closing the gap between "a cold boot is fast" and "a workload is actually ready to do something": see [Startup latency & the pre-warmed pool](../startup-latency/) for the measured ~25-100x difference between a cold sandbox's first real command and a resumed one's. sandkiln builds three distinct operations on top of the same underlying mechanism, because "boot from a snapshot" turns out to have two genuinely different use cases that need different consumption rules.

## Key terms

| Term | Meaning here |
| --- | --- |
| `pause` / `snapshot` / `resume` | Firecracker's own three-step API: pause the vCPUs, dump state to disk, later load that state into a new process. |
| **Consuming** vs. **non-consuming** | Whether the operation retires the snapshot record afterward. `resume` consumes it (a second resume 404s); `fork` doesn't (the same snapshot can be forked again). |
| `forked_into` | The lock a `Snapshot` record carries while a live fork exists, sandkiln's own answer to "Firecracker has no mechanism to give two live descendants independent guest identities," described below. |
| **Retired checkpoint** | What a consumed snapshot becomes by default instead of being deleted outright. See "Time-travel restore" below. |
| `parent_snapshot_id` | The lineage pointer every snapshot carries: which snapshot its own source sandbox was itself resumed or forked from, `null` for a lineage root. |

## How it works in sandkiln

`Vm::pause`/`Vm::snapshot`/`Vm::resume` (`core/crates/vmm/src/vm/snapshot.rs`) are thin wrappers over Firecracker's own `/vm` (state: Paused) and `/snapshot/create` API calls, nothing more than that at the hypervisor layer. Everything about *which* files, *whose* lease, and *what happens after* is the daemon's own design, in `core/crates/daemon/src/routes_snapshot.rs` and `snapshot.rs`.

**Resume** (`resume_snapshot_by_id`) builds a brand-new `Sandbox` record that owns a fresh rootfs clone and its original network lease outright: structurally identical to a cold `POST /sandboxes` in everything except how the VM itself boots. The snapshot record is consumed, so a second resume of the same id returns `404`.

**Fork** (`fork_snapshot`) boots a second live sandbox from the same snapshot *without* consuming it. This is where the network-identity constraint above becomes a real design decision rather than a footnote: Firecracker has no verified mechanism to give two live descendants of one snapshot independent guest IP/MAC, since both would resume believing they own the identity frozen into that one memory image. Rather than allow that and produce a silent network collision, `Snapshot::forked_into` is a lock, set while a fork is live, checked by `fork_snapshot`, `resume_snapshot`, `delete_snapshot`, and `snapshot_sandbox` alike, and cleared only once that fork's `Vm` is actually killed. A second `fork`/`resume` attempt while one is already live gets a real `409`, not silent corruption.

**A real bug, and what it teaches about the rootfs-path rule above.** For a while, a forked sandbox shared its source snapshot's rootfs *file* directly, not a copy, the literal same path. `forked_into`'s lock made that look safe as long as nothing was ever *concurrently* live twice. But the rule above ("resume reopens whatever's at the recorded path right now") doesn't care whether the violation is concurrent or sequential. Fork a snapshot, let the fork mutate the shared file, stop the fork, then resume the *original* snapshot directly: its device state still describes the rootfs from before the fork ever ran, but the file on disk now carries the fork's changes. This was reproduced live, not found by re-reading code. Write a marker, fork, overwrite the marker in the fork, stop the fork, resume the original, and the marker comes back with the fork's value. The fix follows directly from the file-path rule stated above: a fork now gets its own private rootfs clone (`cp --reflink=auto`, the same call a fresh `POST /sandboxes` already uses), so nothing a fork does can ever reach back into what the snapshot's next resume sees.

**Time-travel restore.** Resume's "the snapshot is gone once you move forward from it" used to be unconditional. By default now, `POST /snapshots/:id/resume` still consumes the snapshot *record* the same way, but retires the underlying checkpoint into `GET /snapshots/history` instead of deleting its files, restorable again later via `POST /snapshots/history/:id/restore`, as many times as wanted, since restoring doesn't consume it either (`?retain_history=false` opts back into the fully-destructive original behavior). A restored sandbox owns a fresh rootfs clone and lease outright, exactly like a resume, which is also why it stays snapshottable afterward, unlike a fork. The same one-live-descendant lock generalizes across a whole lineage's history: restoring is refused with `409` while anything else sharing that checkpoint's frozen network identity is currently live.

## See it in action

Fork the same snapshot twice in a row. This is the daemon's own concurrency lock, captured live:

```
$ curl -s -X POST http://127.0.0.1:7777/sandboxes \
    -H 'content-type: application/json' -d '{"tags":{"purpose":"docs-example"}}'
{"id":"5ce396c6-a633-4262-b1a5-b307674a870c"}

$ curl -s -X POST http://127.0.0.1:7777/sandboxes/5ce396c6-a633-4262-b1a5-b307674a870c/snapshot
{"snapshot_id":"b53b17e7-8211-4a80-bc23-bbe6111f6e11"}

$ curl -s -X POST http://127.0.0.1:7777/snapshots/b53b17e7-8211-4a80-bc23-bbe6111f6e11/fork
{"id":"b94d7a70-94c5-4162-b87b-25f0a03b951e"}

$ curl -si -X POST http://127.0.0.1:7777/snapshots/b53b17e7-8211-4a80-bc23-bbe6111f6e11/fork
HTTP/1.1 409 Conflict
content-type: application/json
x-request-id: d0d4aeab-df13-4936-9eaa-f14a6d3d769e
content-length: 153

{"error":"snapshot b53b17e7-8211-4a80-bc23-bbe6111f6e11 has a live fork (b94d7a70-94c5-4162-b87b-25f0a03b951e) — stop it before forking this snapshot"}
```

The second fork attempt is rejected while the first one is still alive. That's the `forked_into` lock described above, not a hypothetical.

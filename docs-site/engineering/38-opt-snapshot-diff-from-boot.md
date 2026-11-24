---
title: "Snapshot take: from a whole memory file to the touched pages"
sidebarTitle: "Snapshot take, touched pages only"
description: "A snapshot of a fresh 512 MiB guest wrote and fsynced all 512 MiB. Splitting the take into steps and tracing Firecracker showed the cost was the memory file, not sandkiln. A cold-booted sandbox is now snapshotted as a diff of the pages it touched: the suite median went from 1781 ms to 546 ms. This post covers the measurements, the three levers weighed and not built, and what stays correct."
---

[What a snapshot costs](/engineering/23-opt-snapshot-cost-analysis) recorded a baseline and said nothing had shipped. This is the follow-up: where a take actually went, what changed, and what did not.

## Problem and how it was found

A snapshot of a freshly booted 512 MiB sandbox took 0.4 to 1.5 s in the benchmark suite, with a median of 1.2 s. It sets how fast the warm pool refills (every claim triggers a boot and a snapshot in the background) and how heavy a time-travel checkpoint is.

The first step was to make the take measurable. `snapshot_and_stop` now records `snapshot_take_phase_duration_ms{phase}` for pause, create (Firecracker's `PUT /snapshot/create`), force stop, lease release, metadata persist and the total, and `scripts/snapshot-profile.sh` prints them per take. On a 512 MiB fresh guest, medians over 12 takes:

| Step | Median |
| --- | ---: |
| pause | 0.5 ms |
| create (Firecracker) | 490 ms |
| force stop | 31 ms |
| release the lease | 18 ms |
| persist `meta.json` | 2 ms |
| total | 540 ms |

Sandkiln's own steps are about 50 ms. The rest is Firecracker, and it varied by a factor of five between consecutive takes (364 to 1845 ms), which is where the old 310 to 990 ms range came from.

## Root cause

A strace of the Firecracker process during one take, on Firecracker v1.16.1:

| Syscall | Time |
| --- | ---: |
| `fsync` of the rootfs drive file | 42 ms |
| `fsync` of the state file | 3 ms |
| one `write` of 512 MiB, zeros included | 184 ms |
| `fsync` of the memory file | 819 ms |

The memory file is written whole, not sparse: even a fresh guest that has touched about 90 MiB leaves 512 MiB allocated. The write is a copy into the page cache; the fsync is the disk. Pointing the state directory at tmpfs, where fsync is free, took a take from 540 ms to 242 ms, which confirms the split. A 1 GiB guest took 3.7 s.

So a take scaled with configured RAM and with disk state, not with anything the guest had done.

## The change

Firecracker can write a Diff snapshot: only the pages dirtied since the last snapshot, using KVM dirty-page tracking that must be switched on when the VM is configured. For a VM that has never been snapshotted, a diff is every page the guest touched since boot, and the memory file is sparse everywhere else. A booted guest's untouched pages are zero and a hole reads as zero, so that file is a complete memory image. Firecracker's snapshot documentation calls diffs of booted VMs the resumable exception.

So no base snapshot and no merge chain are needed for the common case:

- A VM boots with `track_dirty_pages` when `snapshot_dirty_tracking` is on (the default).
- `Vm::snapshot` asks a tracked VM for `snapshot_type: Diff`.
- A resumed VM is never tracked: its memory came from a file, so "dirtied since boot" says nothing about what differs from that file. It is snapshotted in full as before.

The diff is only as good as the tracking. Pages written by a device instead of the guest CPU, such as the block device filling the page cache, are the case most likely to be missed. The integration topic `34-snapshot-memory-integrity` writes 64 MiB of random data into guest memory, reads 1500 files from disk into its page cache, takes a snapshot, then compares checksums in a fork, in a resume, and in a resume of a full snapshot taken from that resumed sandbox. All pass, and so did the full integration suite (458 checks).

## Result

Fresh guest, full write against diff, alternated so disk drift hits both, 6 takes each:

| Guest RAM | Full | Diff |
| ---: | ---: | ---: |
| 256 MiB | 369 ms | 168 ms |
| 512 MiB | 620 ms | 152 ms |
| 1024 MiB | 3679 ms | 376 ms |

A diff scales with what the guest touched, so the gap narrows as it dirties more: a 512 MiB guest with 128 MiB dirtied took 590 ms full and 293 ms diff. The diff was never slower in these runs. The memory file of a fresh 512 MiB guest has 92 MiB allocated instead of 512.

The benchmark suite, `--only snapshot` (create, one command, snapshot), pool disabled, three alternating rounds of 20 takes:

| | Median | p95 |
| --- | ---: | ---: |
| Before | 1781 ms | 3380 ms |
| After | 546 ms | 775 ms |

Resume and fork did not move (resume 19.2 ms to 19.7 ms). After a page-cache drop, the first resume read 138 ms from the full file and 85 ms from the sparse one, because holes need no disk read.

The per-take figures come from `2026-11-24-snapshot-take-experiments-i9-13900K.txt`, the suite figures from the `snapshot-full` and `snapshot-diff` result files, both in `examples/benchmark-suite/results/`. They are one shared box and directional.

## What was weighed and not built

| Lever | Why not |
| --- | --- |
| `sync_snapshot_files=false` | Firecracker 1.17.0 added it; the box's v1.16.1 rejects the field. Measured on 1.17.0 with a temporary build: a full write drops from 566 to 1222 ms to about 300 ms, but on top of the diff it saves only 20 to 110 ms of 150 to 260, and the later `meta.json` fsync absorbs part of the flush. It needs a Firecracker upgrade for every install, and a listed snapshot with an unsynced memory file would not survive a power loss intact. |
| A chain of diffs for repeated takes | A resumed sandbox would need its base, a merge and a lifetime rule across resume, fork, restore, archive and delete. The cold-booted case needed none of that. |
| Keeping the file hot in the page cache | A take leaves the file in the page cache already. The cold-cache cost appears only after eviction or a reboot. |

## What it costs, and what is left

Tracking has no measurable runtime cost here: cold create to first command was 229 ms with tracking off and 228 ms on (60 creates each), and a guest wrote 400 MiB to tmpfs in 256 ms either way. It is not measured for a guest that rewrites a large working set for a long time. Upstream calls diff snapshots a developer preview, so `snapshot_dirty_tracking = false` restores full writes.

About 280 ms of a take made right after create is `pause` waiting until the guest is 500 ms old, a guard added after snapshots of younger guests killed half their resumes ([resume settling race](/engineering/30-correctness-resume-settling-race)). It is now the largest single piece, and it was left alone: a snapshot that cannot resume is permanent, and the sweep that justified 500 ms had 60 samples per point.

## How to reproduce

```bash
scripts/dev.sh snapshot-profile --runs 12
scripts/dev.sh snapshot-profile --mem 1024 --dirty 256
scripts/dev.sh bench-suite --only snapshot --iterations 20
```

Run with `default_warm_count = 0` on an idle daemon, and alternate `SANDKILN_SNAPSHOT_DIRTY_TRACKING=false` with the default. The design note [`docs/design-notes/snapshot-take-cost.md`](https://github.com/SumitKumar-17/sandkiln/blob/main/docs/design-notes/snapshot-take-cost.md) has every table.

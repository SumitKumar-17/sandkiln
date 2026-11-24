# Snapshot take cost

Why taking a snapshot cost 0.4 to 1.5 s, what was measured, what shipped and
what was weighed and not built. Raw runs:
[`examples/benchmark-suite/results/2026-11-24-snapshot-take-experiments-i9-13900K.txt`](../../examples/benchmark-suite/results/2026-11-24-snapshot-take-experiments-i9-13900K.txt)
(experiments E1 to E7, quoted below by number) and the suite results named in
[current numbers](../../docs-site/performance/current-numbers.md). One shared
i9-13900K box, Firecracker v1.16.1, kernel 6.8, guest kernel 5.10.223, state
directory on the ext4 root disk, default warm pool off. Directional, like every
figure here.

## Decision

A sandbox that was cold-booted is snapshotted as a Firecracker diff: only the
pages the guest touched go to disk, the memory file is sparse. Guests boot with
KVM dirty-page tracking for this (`snapshot_dirty_tracking`, on by default). A
resumed sandbox, which has no clean "since boot" baseline, is still snapshotted
in full. A fresh 512 MiB guest takes about 150 ms of snapshot work instead of
about 600 to 1200 ms, and the cost no longer grows with configured RAM.

Not built: skipping the file sync, a chain of diffs, a page-cache step. Reasons
per lever below.

## Where a take went (E1, E1b, E2, E5)

512 MiB fresh guest, full write, 12 runs, medians in ms (`scripts/snapshot-profile.sh`):

| Step | Median | What it is |
| --- | ---: | --- |
| pause | 0.5 | `PATCH /vm`. 282 ms instead when the guest is under 500 ms old (E1b): `MIN_GUEST_AGE_BEFORE_PAUSE`, a correctness wait, finding 20 |
| create | 490 | Firecracker's `PUT /snapshot/create` |
| force_stop | 31 | SIGKILL and reap |
| release | 18 | egress chain removal and two `ip` calls to return the tap |
| persist | 2 | atomic `meta.json` |
| total | 540 | |

So sandkiln's own steps are about 50 ms (9 percent) and the rest is Firecracker.
Inside `create`, a strace of one full take (E5) shows:

| Syscall in Firecracker | Time |
| --- | ---: |
| `fsync` of the rootfs drive file (always done) | 42 ms |
| `fsync` of `state.snap` | 3 ms |
| one `write` of all 512 MiB, zeros included | 184 ms |
| `fsync` of the memory file | 819 ms |

The memory file is written in full and not sparse: a fresh 512 MiB guest still
leaves 512 MiB allocated on disk (`2026-11-24-snapshot-file-sizes-i9-13900K.txt`). The write itself is a memcpy into the
page cache; the flush is the disk. Putting the state directory on tmpfs (E2),
where fsync is free, leaves 194 ms in `create` and 242 ms in total, against 490
and 540 on the ext4 root disk. The XFS reflink volume was not faster (727 ms
total, E2). The 310 to 990 ms range quoted before was this flush meeting
different disk states: consecutive runs on one daemon ranged from 364 to 1845 ms
of `create` in E1.

## Size and dirtiness (E3, E4)

Full write against diff, alternated so disk drift hits both, 6 runs each, total ms:

| Guest RAM (fresh) | Full | Diff |
| ---: | ---: | ---: |
| 128 MiB | 296 | 213 |
| 256 MiB | 369 | 168 |
| 512 MiB | 620 | 152 |
| 1024 MiB | 3679 | 376 |

| Dirty MiB requested in a 512 MiB guest | Full | Diff |
| ---: | ---: | ---: |
| 0 (fresh) | 743 | 161 |
| 128 | 590 | 293 |
| 256 | 587 | 468 |
| 400 | 1103 | 457 |

The guest's tmpfs is half its RAM, so a request above 256 MiB writes about 256;
the last two rows are the same load. A full take scales with configured RAM and
with how noisy the disk is. A diff scales with what the guest touched: the
memory file of a fresh 512 MiB guest has 92 MiB allocated after a diff, 219 MiB
after 128 MiB of dirtying and 334 MiB after the 400 MiB request, against 512 MiB
for every full take (same file as above). The diff is never worse than the full write in these runs;
the gap closes as the guest dirties more, which is expected, and a guest that
dirtied everything would write about as much as before plus the dirty bitmap.

## Levers

| Lever | Measured | Decision |
| --- | --- | --- |
| Diff snapshot of a cold-booted guest | 620 to 152 ms (512 MiB), 3679 to 376 ms (1 GiB); suite median 1781 to 546 ms, p95 3380 to 775 ms | **Built.** Works on Firecracker v1.16.1, no new dependency |
| (a) `sync_snapshot_files=false` | Needs Firecracker 1.17.0 or later (v1.16.1 rejects the field: its error lists `snapshot_type`, `snapshot_path`, `mem_file_path`; 1.17.0 and 1.18.0 add it, [CHANGELOG 1.17.0](https://github.com/firecracker-microvm/firecracker/blob/main/CHANGELOG.md), [snapshot docs](https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/snapshot-support.md)). Full write: 1222 and 566 ms with sync, 297 and 302 without (E6). On top of a diff: 172 and 258 ms with sync, 153 and 146 without | **Not built.** The gain over the diff is 20 to 110 ms of 150 to 260, and `persist` rises from 1 to 5 ms to 13 to 24 ms because the later `meta.json` fsync absorbs part of the flush. It would need a Firecracker upgrade for every install and a durability split: the docs promise snapshots survive a host reboot, a clean reboot flushes the page cache, but a power loss with an fsynced `meta.json` and an unsynced `mem.bin` would leave a snapshot that is listed and cannot resume. Reconsider with a Firecracker upgrade for other reasons, for warm-pool entries only |
| (b) Diff chain for repeated takes | Not measured: it needs `track_dirty_pages` on `/snapshot/load`, a base snapshot and `snapshot-editor` merges | **Not built.** A resumed sandbox is snapshotted in full. The chain changes what "a snapshot is two files" means for resume, fork, time-travel restore, archive and delete of an intermediate snapshot. Worth revisiting only if repeated take of one long-lived sandbox becomes the workload |
| (c) Keep the memory file in the page cache | A take leaves the file in the page cache (it was just written). First resume with the cache warm: 17 ms (full file), 22 ms (diff file); after dropping caches: 138 ms and 85 ms (E7) | **Nothing to build for the take.** The cold-cache penalty (about 120 ms for the full file, 60 for the sparse one) appears only after eviction or a reboot; a readahead on a cold claim is a separate, untested idea |
| (d) Smaller default memory for pool entries | not allowed | Not done |
| (e) Everything else | Settling wait 282 ms (right after create). Firecracker's rootfs fsync 42 ms. `force_stop` 31 ms, `release` 18 ms | See below |

## What is left in a take

After the change a take right after create has a suite median of 546 ms, of which about 280 ms is `pause` waiting for the guest to be 500 ms old.
That wait exists because a guest paused earlier produced snapshots that killed
50 percent of resumes (finding 20); the sweep that justified it measured 0 of 60
at 0.1 s of delay, but one resumed snapshot that cannot start is permanent, so
the constant was left alone. Lowering it needs its own sweep with far more
samples than this work took. It is now the largest single piece, and the one
place a take is slower than it needs to be for a reason other than hardware.

The rest is about 100 ms of Firecracker (rootfs fsync plus the dirty pages), 31
ms of process teardown and 18 ms for two `ip` subprocesses. None of these was
attacked.

## How the diff stays correct

- A VM is dirty-tracked only if it booted that way (`Vm::dirty_tracked`). A
  resumed VM never is: its memory came from a file, so "dirtied since boot" says
  nothing about what differs from the file. `Vm::snapshot` asks for a `Diff` only
  for a tracked VM (unit tests: `snapshot_request`).
- The diff of a booted VM with no earlier snapshot is a complete memory image:
  untouched pages are zero and a hole reads as zero. Firecracker's own docs say
  diffs of booted VMs are the resumable exception.
- Pages written by devices rather than the guest CPU (the block device filling
  the guest's page cache) are the case dirty tracking could get wrong. The test
  `34-snapshot-memory-integrity.sh` reads data that lives only in guest memory
  (64 MiB of random tmpfs data, md5 of 1500 files read from disk) before a take
  and after a resume, a fork and a second, full take of the resumed sandbox, and
  compares checksums. It passes with tracking on.
- Moving a sparse `mem.bin` between filesystems (the archive tier's copy
  fallback) writes the holes out as zeros: larger, still correct.
- `snapshot_dirty_tracking=false` restores the old behaviour. Upstream diff
  snapshots are a developer preview (the snapshot docs say so), which is why the
  switch exists.

## What was not verified

- Many thousands of resumes of diff snapshots. The integration suite and the
  suite runs resumed a few hundred, all healthy.
- Upstream says dirty tracking "comes at a cost". Measured here, alternating off
  and on: cold create to first command 229 ms and 228 ms (60 creates each, see
  [current numbers](../../docs-site/performance/current-numbers.md)), and a guest
  writing 400 MiB to its tmpfs in 256.0 and 256.5 ms median (24 runs each,
  `2026-11-24T2026-dirty-tracking-runtime-i9-13900K.txt`). Not measured: a guest
  that rewrites a large working set repeatedly for a long time before a take.
- Firecracker versions other than v1.16.1 for the diff itself. Only the
  `sync_snapshot_files` experiment used 1.17.0.
- Jailed sandboxes, which cannot be snapshotted yet.

## Reproduce

```bash
scripts/dev.sh snapshot-profile --runs 12              # the steps of a take
scripts/dev.sh snapshot-profile --mem 1024 --dirty 256
scripts/dev.sh bench-suite --only snapshot --iterations 20
```

Run them with `default_warm_count = 0` on an idle daemon; compare
`SANDKILN_SNAPSHOT_DIRTY_TRACKING=false` against the default, alternating.
Engineering post: [snapshot take, from a whole memory file to the touched
pages](/engineering/38-opt-snapshot-diff-from-boot).

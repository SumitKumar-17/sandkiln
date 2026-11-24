# Further latency investigation

Open questions in priority order, chosen because a measurement or a
documented fact could answer them.

1. **Reflink as the default.** The `clone_rootfs` destination bug had to be
   fixed first, since every later latency number would otherwise sit on a
   baseline known to be wrong; finding 4 shipped the mechanism and metric.
   Remaining: decide on production defaults; `create-reflink-volume.sh` is a loop mount
   and does not survive a reboot, so add a systemd mount unit.
2. **Snapshot-take latency.** Measured and mostly fixed: the take was split
   into steps (`snapshot_take_phase_duration_ms`), the cost turned out to be
   Firecracker writing and fsyncing the whole memory file (not sandkiln's
   wrapping, which is about 50 ms), and a cold-booted sandbox is now
   snapshotted as a Firecracker diff of only the pages it touched. Evidence,
   the levers weighed and the decisions: [snapshot take cost](snapshot-take-cost.md).
   Open there: the guest settling wait is now the largest part of a take right
   after create, and `sync_snapshot_files` needs Firecracker 1.17.
3. **Firecracker Diff snapshots.** Built for the cold-booted case (no base
   snapshot, no merge chain): [snapshot take cost](snapshot-take-cost.md). A
   chain of diffs for repeated takes of one sandbox is not built.
4. **Guest userspace boot.** An earlier note said guest boot was not a
   priority: it sat inside the roughly 10ms cold-boot window, and the
   measured 10.5-10.9ms was already far ahead of public Firecracker-sandbox
   cold starts (150ms to 2s), so there was no evidence of a gain to chase. After counting through to first command, the
   remaining floor is about 125ms of systemd userspace in our guest, not to be confused with Firecracker's 125ms init bound (`dev-vda.device`
   107ms, `fcnet.service` 45ms), so a purpose-built init or trimmed image is
   now the next lever for time to first command. The stage-by-stage gap
   analysis and ranked levers are in [latency floor](latency-floor.md).
5. **Memory compression during snapshot-take.** One academic result reports
   up to 4.5x smaller snapshots and 55% faster restoration. Much further
   out; only after items 1-3.
6. **A rootfs/CoW crate with an explicit dirty bitmap.** See
   [ideas not started](ideas-not-started.md): a smaller, more concrete step
   than Diff snapshots' bookkeeping alternative, and the same bookkeeping a
   cheaper rootfs snapshot would need (take is full memory and disk each
   time).

Settled: a netlink crate to replace `ip`/`bridge` subprocess calls is not
worth it (the lease is about 4ms and concurrent; see
[benchmarks](../benchmarks.md)).

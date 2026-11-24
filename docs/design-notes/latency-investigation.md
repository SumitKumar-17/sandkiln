# Further latency investigation

Open questions in priority order, chosen because a measurement or a
documented fact could answer them.

1. **Reflink as the default** (finding 4 shipped the mechanism and metric).
   Decide on production defaults; `create-reflink-volume.sh` is a loop mount
   and does not survive a reboot, so add a systemd mount unit.
2. **Snapshot-take latency** (309.6-990ms, the most load-sensitive figure).
   Firecracker's docs confirm the asymmetry is structural: take writes every
   dirty page synchronously, resume loads memory lazily via copy-on-write.
   The open question is how much sandkiln's wrapping adds on top of an
   inherently expensive operation; unmeasured.
3. **Firecracker Diff snapshots.** Capture only pages dirtied since the last
   snapshot instead of a full dump. At least one other small project chains
   about ten diffs before collapsing to a full snapshot. Needs measurement
   on this workload (frequent snapshot/resume of one sandbox) before
   claiming a win.
4. **Guest userspace boot.** An earlier note said guest boot was not a
   priority at about 10ms. After counting through to first command, the
   remaining floor is about 125ms of systemd userspace (`dev-vda.device`
   107ms, `fcnet.service` 45ms), so a purpose-built init or trimmed image is
   now the next lever for time to first command. Public comparisons put
   typical Firecracker sandbox cold starts at 150ms to 2s.
5. **Memory compression during snapshot-take.** One academic result reports
   up to 4.5x smaller snapshots and 55% faster restoration. Much further
   out; only after items 1-3.
6. **A rootfs/CoW crate with an explicit dirty bitmap.** See
   [ideas not started](ideas-not-started.md).

Settled: a netlink crate to replace `ip`/`bridge` subprocess calls is not
worth it (the lease is about 4ms and concurrent; see
[benchmarks](../benchmarks.md)).

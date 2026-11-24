# Further latency investigation

Open questions in priority order, chosen because a measurement or a
documented fact could answer them.

1. **Reflink as the default.** The `clone_rootfs` destination bug had to be
   fixed first, since every later latency number would otherwise sit on a
   baseline known to be wrong; finding 4 shipped the mechanism and metric.
   Remaining: decide on production defaults; `create-reflink-volume.sh` is a loop mount
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
   priority: it sat inside the roughly 10ms cold-boot window, and the
   measured 10.5-10.9ms was already far ahead of public Firecracker-sandbox
   cold starts (150ms to 2s), so there was no evidence of a gain to chase. After counting through to first command, the
   remaining floor is about 125ms of systemd userspace (`dev-vda.device`
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

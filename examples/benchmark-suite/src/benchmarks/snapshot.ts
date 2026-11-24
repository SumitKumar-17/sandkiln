import { Sandbox } from "sandkiln";
import { type Benchmark, timed } from "../context.ts";

/** One warmed sandbox per iteration goes through snapshot, fork, then resume.
 * The sandbox answers a command before the snapshot so the guest agent is
 * known-live; the resumed and forked first commands then measure restore
 * cost, not agent start-up. The fork is stopped before resuming because only
 * one live fork per snapshot is allowed, and resume consumes the snapshot, so
 * nothing is left behind on the success path. */
export const snapshotBenchmark: Benchmark = {
  name: "snapshot",
  description: "snapshot take, fork + its first command, resume + its first command",
  async run(ctx) {
    for (let i = 0; i < ctx.options.iterations; i++) {
      try {
        const sandbox = await ctx.create();
        await sandbox.runCommand("true");

        const snap = await timed(() => sandbox.snapshot());
        ctx.record("snapshot_take", snap.ms);

        const fork = await timed(() => Sandbox.fork(snap.value, ctx.connection));
        ctx.record("fork_call", fork.ms);
        ctx.record("fork_first_command", (await timed(() => fork.value.runCommand("true"))).ms);
        await fork.value.stop({ keep: false });

        const resume = await timed(() => Sandbox.resume(snap.value, ctx.connection));
        ctx.record("resume_call", resume.ms);
        ctx.record("resume_first_command", (await timed(() => resume.value.runCommand("true"))).ms);
        await resume.value.stop({ keep: false });
      } finally {
        await ctx.cleanup();
      }
    }
  },
};

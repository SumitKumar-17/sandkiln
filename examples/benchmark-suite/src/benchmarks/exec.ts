import { type Benchmark, timed } from "../context.ts";

const WARMUP = 10;

/** Steady-state cost of one command on an already-open sandbox: HTTP call,
 * daemon, vsock, guest agent, fork/exec of `true`, and back. */
export const execBenchmark: Benchmark = {
  name: "exec",
  description: "exec round trip on one open sandbox, after a warmup, over --exec-samples calls",
  async run(ctx) {
    const sandbox = await ctx.create();
    try {
      for (let i = 0; i < WARMUP; i++) await sandbox.runCommand("true");
      for (let i = 0; i < ctx.options.execSamples; i++) {
        ctx.record("exec_roundtrip", (await timed(() => sandbox.runCommand("true"))).ms);
      }
    } finally {
      await sandbox.stop({ keep: false });
    }
  },
};

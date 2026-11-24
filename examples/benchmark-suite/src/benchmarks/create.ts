import { type Benchmark, timed } from "../context.ts";

/** The metric a caller feels: wall time from calling create to a real command
 * answering. `create()` returning only means Firecracker started, not that the
 * guest agent is listening, so the two are recorded separately. */
export const createBenchmark: Benchmark = {
  name: "create",
  description: "create call, first command after create returns, and the two combined (sequential, cold)",
  async run(ctx) {
    for (let i = 0; i < ctx.options.iterations; i++) {
      const created = await timed(() => ctx.create());
      const sandbox = created.value;
      try {
        const first = await timed(() => sandbox.runCommand("true"));
        ctx.record("create_call", created.ms);
        ctx.record("first_command_after_create", first.ms);
        ctx.record("create_to_first_command", created.ms + first.ms);
      } finally {
        await sandbox.stop({ keep: false });
      }
    }
  },
};

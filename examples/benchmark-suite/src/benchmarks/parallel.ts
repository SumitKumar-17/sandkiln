import type { Sandbox } from "sandkiln";
import { type Benchmark, timed } from "../context.ts";

/** `--concurrency` creates fired at once, each followed by one real command,
 * repeated `--iterations` rounds. Throughput is creates that were ready (first
 * command answered) per second of round wall time. */
export const parallelBenchmark: Benchmark = {
  name: "parallel",
  description: "C simultaneous creates, each to first command: per-create latency and throughput",
  async run(ctx) {
    const c = ctx.options.concurrency;
    for (let round = 0; round < ctx.options.iterations; round++) {
      const sandboxes: Sandbox[] = [];
      try {
        const start = performance.now();
        const results = await Promise.all(
          Array.from({ length: c }, async () => {
            const created = await timed(() => ctx.create());
            sandboxes.push(created.value);
            const first = await timed(() => created.value.runCommand("true"));
            return created.ms + first.ms;
          }),
        );
        const wallMs = performance.now() - start;
        for (const ms of results) ctx.record(`parallel_create_to_first_command_c${c}`, ms);
        ctx.record(`parallel_round_wall_c${c}`, wallMs);
        ctx.record(`parallel_throughput_c${c}`, (c / wallMs) * 1000, "ops/s", true);
      } finally {
        await Promise.allSettled(sandboxes.map((s) => s.stop({ keep: false })));
        await ctx.cleanup();
      }
    }
  },
};

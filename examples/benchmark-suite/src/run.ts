/** Orchestrates a run: busy check, environment, benchmarks, result assembly. */

import { randomBytes } from "node:crypto";
import type { RunArgs } from "./args.ts";
import { BENCHMARKS } from "./benchmarks/index.ts";
import { BenchContext } from "./context.ts";
import { collectEnvironment } from "./env.ts";
import { getStatus } from "./status.ts";
import { summarize } from "./stats.ts";
import type { BenchmarkResult } from "./types.ts";

export async function runBenchmarks(args: RunArgs, onSignal: (cleanup: () => Promise<void>) => void): Promise<BenchmarkResult> {
  const unknown = args.only.filter((n) => !BENCHMARKS.some((b) => b.name === n));
  if (unknown.length > 0) throw new Error(`unknown benchmark(s): ${unknown.join(", ")} (see --list)`);
  const selected = BENCHMARKS.filter((b) => args.only.length === 0 || args.only.includes(b.name));

  const connection = { baseUrl: args.baseUrl.replace(/\/$/, ""), authToken: args.authToken };
  const options = { iterations: args.iterations, pauseMs: args.pauseMs, concurrency: args.concurrency, execSamples: args.execSamples, imageId: args.imageId };

  // Fail fast and clearly if the daemon is unreachable or busy: other
  // sandboxes compete for the same disk, taps and CPU and skew every number.
  const status = await getStatus(connection);
  if (status.counts.sandboxes > 0 && !args.allowBusy) {
    throw new Error(`daemon has ${status.counts.sandboxes} live sandbox(es); numbers would be noisy. Retry when idle or pass --allow-busy.`);
  }

  const ctx = new BenchContext(connection, options, `bench-${Date.now().toString(36)}-${randomBytes(3).toString("hex")}`);
  onSignal(() => ctx.cleanup());

  try {
    const environment = await collectEnvironment(ctx, args.hardware);
    for (const bench of selected) {
      console.error(`running ${bench.name}: ${bench.description}`);
      await bench.run(ctx);
    }
    const metrics = Object.fromEntries(
      Object.entries(ctx.metrics).map(([name, m]) => [name, { ...m, summary: summarize(m.samples) }]),
    );
    return { schema: 1, options, environment, metrics };
  } finally {
    await ctx.cleanup();
  }
}

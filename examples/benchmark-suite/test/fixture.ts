import { summarize } from "../src/stats.ts";
import type { BenchmarkResult } from "../src/types.ts";

/** Builds a result from raw samples; names ending `_throughput` count as ops/s. */
export function fixture(samples: Record<string, number[]>): BenchmarkResult {
  const metrics: BenchmarkResult["metrics"] = {};
  for (const [name, values] of Object.entries(samples)) {
    const throughput = name.endsWith("_throughput");
    metrics[name] = { unit: throughput ? "ops/s" : "ms", higherIsBetter: throughput, samples: values, summary: summarize(values) };
  }
  return {
    schema: 1,
    options: { iterations: 3, concurrency: 2, execSamples: 2 },
    environment: {
      date: "2026-11-24T00:00:00.000Z",
      daemonUrl: "http://127.0.0.1:7777",
      daemonVersion: "0.0.1",
      daemonUptimeSecs: 1,
      hardwareLabel: null,
      hostKernel: null,
      hostCpu: null,
      guestKernel: null,
      rootfsClones: {},
      liveSandboxesAtStart: 0,
      node: "v22",
    },
    metrics,
  };
}

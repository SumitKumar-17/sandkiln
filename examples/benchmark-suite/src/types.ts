/** Shapes shared by the runner, the formatters and `compare`. The JSON
 * result file is exactly `BenchmarkResult`, so it is the diffable contract. */

export type Unit = "ms" | "ops/s";

/** Raw measurements for one named metric, in the order they were taken. */
export interface Metric {
  unit: Unit;
  /** Throughput metrics improve upward; latencies improve downward. */
  higherIsBetter: boolean;
  samples: number[];
}

export interface Summary {
  n: number;
  min: number;
  max: number;
  mean: number;
  median: number;
  p95: number;
  p99: number;
}

export interface Environment {
  date: string;
  daemonUrl: string;
  daemonVersion: string;
  daemonUptimeSecs: number;
  /** Free-form label from `--hardware`, e.g. "i9-13900K, 32 threads". */
  hardwareLabel: string | null;
  /** Only filled when the daemon is on this machine (loopback URL). */
  hostKernel: string | null;
  hostCpu: string | null;
  /** Read from inside a sandbox, so it is always available. */
  guestKernel: string | null;
  rootfsClones: Record<string, number>;
  liveSandboxesAtStart: number;
  node: string;
}

export interface RunOptions {
  iterations: number;
  concurrency: number;
  execSamples: number;
}

export interface BenchmarkResult {
  schema: 1;
  options: RunOptions;
  environment: Environment;
  metrics: Record<string, Metric & { summary: Summary }>;
}

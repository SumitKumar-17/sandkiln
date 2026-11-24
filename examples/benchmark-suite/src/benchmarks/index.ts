import type { Benchmark } from "../context.ts";
import { createBenchmark } from "./create.ts";
import { execBenchmark } from "./exec.ts";
import { parallelBenchmark } from "./parallel.ts";
import { snapshotBenchmark } from "./snapshot.ts";

/** Registration order is run order. Adding a benchmark = one file + one line here. */
export const BENCHMARKS: readonly Benchmark[] = [createBenchmark, execBenchmark, snapshotBenchmark, parallelBenchmark];

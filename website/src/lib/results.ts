import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

// The recorded benchmark runs are the only source of the numbers on this page.
// Read at build time from the repo, so there is no copy to keep in sync.
const RESULTS_DIR = path.resolve(process.cwd(), "..", "examples", "benchmark-suite", "results");

interface Run {
  metrics: Record<string, { samples: number[] }>;
}

function median(samples: number[]): number {
  const sorted = [...samples].sort((a, b) => a - b);
  const mid = sorted.length >> 1;
  return sorted.length % 2 ? sorted[mid]! : (sorted[mid - 1]! + sorted[mid]!) / 2;
}

/** Median (rounded, ms) of `metric` in the newest result file whose name contains `label`. */
export function latestMedian(label: string, metric: string): number {
  const files = readdirSync(RESULTS_DIR)
    .filter((name) => name.endsWith(".json") && name.includes(label))
    .sort();
  const newest = files.at(-1);
  if (!newest) throw new Error(`no benchmark result matching "${label}" in ${RESULTS_DIR}`);
  const run = JSON.parse(readFileSync(path.join(RESULTS_DIR, newest), "utf8")) as Run;
  const samples = run.metrics[metric]?.samples;
  if (!samples?.length) throw new Error(`${newest} has no samples for ${metric}`);
  return Math.round(median(samples));
}

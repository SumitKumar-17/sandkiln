/** Pure statistics. No I/O, so it is unit-tested without a daemon. */

import type { Summary } from "./types.ts";

/** Linear-interpolation percentile (the "type 7" definition used by
 * numpy and most spreadsheets). `p` is 0..100; `sorted` must be ascending. */
export function percentile(sorted: readonly number[], p: number): number {
  if (sorted.length === 0) throw new RangeError("percentile of an empty sample");
  if (p < 0 || p > 100) throw new RangeError(`percentile out of range: ${p}`);
  const rank = (p / 100) * (sorted.length - 1);
  const lo = Math.floor(rank);
  const hi = Math.ceil(rank);
  const loValue = sorted[lo] as number;
  const hiValue = sorted[hi] as number;
  return loValue + (hiValue - loValue) * (rank - lo);
}

export function summarize(samples: readonly number[]): Summary {
  if (samples.length === 0) throw new RangeError("cannot summarize zero samples");
  const sorted = [...samples].sort((a, b) => a - b);
  const sum = sorted.reduce((acc, v) => acc + v, 0);
  return {
    n: sorted.length,
    min: sorted[0] as number,
    max: sorted[sorted.length - 1] as number,
    mean: sum / sorted.length,
    median: percentile(sorted, 50),
    p95: percentile(sorted, 95),
    p99: percentile(sorted, 99),
  };
}

/** Signed percent change from `before` to `after`; `null` when `before` is 0. */
export function percentChange(before: number, after: number): number | null {
  if (before === 0) return null;
  return ((after - before) / before) * 100;
}

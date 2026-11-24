/** Diff two result files. Pure: takes parsed results, returns rows. */

import { formatPercent, formatValue } from "./format.ts";
import { percentChange } from "./stats.ts";
import type { BenchmarkResult } from "./types.ts";

export interface CompareOptions {
  /** A median worse than the baseline by more than this percent is a regression. */
  thresholdPercent: number;
  /** ...and by more than this many milliseconds, so a 40us wobble on a 245us
   * metric is not reported as a 16% regression. Ignored for throughput. */
  minDeltaMs: number;
}

export type Verdict = "regression" | "improvement" | "ok" | "only-before" | "only-after";

export interface CompareRow {
  metric: string;
  before: number | null;
  after: number | null;
  change: number | null;
  verdict: Verdict;
  unit: "ms" | "ops/s";
}

export function compareResults(before: BenchmarkResult, after: BenchmarkResult, opts: CompareOptions): CompareRow[] {
  const names = [...new Set([...Object.keys(before.metrics), ...Object.keys(after.metrics)])];
  return names.map((metric) => {
    const b = before.metrics[metric];
    const a = after.metrics[metric];
    if (!b || !a) {
      return {
        metric,
        before: b?.summary.median ?? null,
        after: a?.summary.median ?? null,
        change: null,
        verdict: b ? "only-before" : "only-after",
        unit: (a ?? b)!.unit,
      } satisfies CompareRow;
    }
    const change = percentChange(b.summary.median, a.summary.median);
    return { metric, before: b.summary.median, after: a.summary.median, change, verdict: classify(a, b, change, opts), unit: a.unit };
  });
}

function classify(a: BenchmarkResult["metrics"][string], b: BenchmarkResult["metrics"][string], change: number | null, opts: CompareOptions): Verdict {
  if (change === null) return "ok";
  // Positive `worse` always means "got worse", whichever way the metric points.
  const worse = a.higherIsBetter ? -change : change;
  const absDelta = Math.abs(a.summary.median - b.summary.median);
  const bigEnough = a.unit === "ms" ? absDelta >= opts.minDeltaMs : true;
  if (worse > opts.thresholdPercent && bigEnough) return "regression";
  if (worse < -opts.thresholdPercent && bigEnough) return "improvement";
  return "ok";
}

export function hasRegression(rows: readonly CompareRow[]): boolean {
  return rows.some((r) => r.verdict === "regression");
}

export function renderCompare(rows: readonly CompareRow[], opts: CompareOptions): string {
  const header = ["metric", "before", "after", "change", "verdict"];
  const body = rows.map((r) => [
    r.metric,
    r.before === null ? "-" : formatValue(r.before, r.unit),
    r.after === null ? "-" : formatValue(r.after, r.unit),
    formatPercent(r.change),
    r.verdict === "regression" ? "REGRESSION" : r.verdict,
  ]);
  const widths = header.map((h, i) => Math.max(h.length, ...body.map((r) => (r[i] as string).length)));
  const line = (cells: string[]) => cells.map((c, i) => (i === 0 || i === 4 ? c.padEnd(widths[i] as number) : c.padStart(widths[i] as number))).join("  ");
  const note = `Medians compared; regression = worse by more than ${opts.thresholdPercent}% and ${opts.minDeltaMs}ms (latency metrics).`;
  return [line(header), widths.map((w) => "-".repeat(w)).join("  "), ...body.map(line), "", note].join("\n");
}

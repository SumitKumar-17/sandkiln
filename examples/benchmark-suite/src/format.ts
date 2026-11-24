/** Rendering: number formatting plus the plain-text and Markdown tables. */

import type { BenchmarkResult, Unit } from "./types.ts";

export function formatValue(value: number, unit: Unit): string {
  if (unit === "ms") {
    // Sub-millisecond figures (exec round trip) need the extra digits.
    if (value < 1) return `${(value * 1000).toFixed(0)}us`;
    if (value < 100) return `${value.toFixed(2)}ms`;
    return `${value.toFixed(1)}ms`;
  }
  return `${value.toFixed(1)}/s`;
}

export function formatPercent(change: number | null): string {
  if (change === null) return "n/a";
  const sign = change > 0 ? "+" : "";
  return `${sign}${change.toFixed(1)}%`;
}

const HEADER = ["metric", "median", "p95", "p99", "min", "max", "n"];

function rows(result: BenchmarkResult): string[][] {
  return Object.entries(result.metrics).map(([name, m]) => {
    const f = (v: number) => formatValue(v, m.unit);
    const s = m.summary;
    return [name, f(s.median), f(s.p95), f(s.p99), f(s.min), f(s.max), String(s.n)];
  });
}

export function renderTable(result: BenchmarkResult): string {
  const body = rows(result);
  const widths = HEADER.map((h, i) => Math.max(h.length, ...body.map((r) => (r[i] as string).length)));
  const line = (cells: string[]) => cells.map((c, i) => (i === 0 ? c.padEnd(widths[i] as number) : c.padStart(widths[i] as number))).join("  ");
  return [line(HEADER), widths.map((w) => "-".repeat(w)).join("  "), ...body.map(line)].join("\n");
}

export function renderEnvironment(result: BenchmarkResult): string {
  const e = result.environment;
  const pairs: [string, string][] = [
    ["date", e.date],
    ["daemon", `${e.daemonUrl} (version ${e.daemonVersion}, up ${e.daemonUptimeSecs}s)`],
    ["hardware", e.hardwareLabel ?? "not given (--hardware)"],
    ["host kernel", e.hostKernel ?? "unknown (daemon is not on this machine)"],
    ["host cpu", e.hostCpu ?? "unknown (daemon is not on this machine)"],
    ["guest kernel", e.guestKernel ?? "unknown"],
    ["rootfs clones", Object.entries(e.rootfsClones).map(([k, v]) => `${k}=${v}`).join(", ") || "none yet"],
    ["live sandboxes at start", String(e.liveSandboxesAtStart)],
    ["node", e.node],
    ["options", `iterations=${result.options.iterations} pause-ms=${result.options.pauseMs ?? 0} concurrency=${result.options.concurrency} exec-samples=${result.options.execSamples} image=${result.options.imageId ?? "default"}`],
  ];
  return pairs.map(([k, v]) => `${k.padEnd(24)} ${v}`).join("\n");
}

export function renderMarkdown(result: BenchmarkResult): string {
  const env = result.environment;
  const table = [
    `| ${HEADER.join(" | ")} |`,
    `| ${HEADER.map((_, i) => (i === 0 ? "---" : "---:")).join(" | ")} |`,
    ...rows(result).map((r) => `| ${r.join(" | ")} |`),
  ].join("\n");
  const envLines = [
    `- Date: ${env.date}`,
    `- Daemon: version ${env.daemonVersion}, ${env.daemonUrl}`,
    `- Hardware: ${env.hardwareLabel ?? "not given"}`,
    `- Host kernel: ${env.hostKernel ?? "unknown"}`,
    `- Host CPU: ${env.hostCpu ?? "unknown"}`,
    `- Guest kernel: ${env.guestKernel ?? "unknown"}`,
    `- Rootfs clones so far: ${Object.entries(env.rootfsClones).map(([k, v]) => `${k}=${v}`).join(", ") || "none"}`,
    `- Live sandboxes at start: ${env.liveSandboxesAtStart}`,
    `- Options: iterations=${result.options.iterations}, pause-ms=${result.options.pauseMs ?? 0}, concurrency=${result.options.concurrency}, exec-samples=${result.options.execSamples}, image=${result.options.imageId ?? "default"}`,
  ].join("\n");
  return `## Environment\n\n${envLines}\n\n## Results\n\n${table}\n`;
}

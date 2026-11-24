#!/usr/bin/env node
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { type Args, USAGE, parseCli } from "./args.ts";
import { BENCHMARKS } from "./benchmarks/index.ts";
import { compareResults, hasRegression, renderCompare } from "./compare.ts";
import { renderEnvironment, renderMarkdown, renderTable } from "./format.ts";
import { runBenchmarks } from "./run.ts";
import type { BenchmarkResult } from "./types.ts";

function readResult(path: string): BenchmarkResult {
  const parsed = JSON.parse(readFileSync(path, "utf8")) as BenchmarkResult;
  if (parsed.schema !== 1 || typeof parsed.metrics !== "object") throw new Error(`${path} is not a benchmark-suite result file`);
  return parsed;
}

async function main(args: Args): Promise<number> {
  switch (args.command) {
    case "help":
      console.log(USAGE);
      return 0;
    case "list":
      for (const b of BENCHMARKS) console.log(`${b.name.padEnd(10)} ${b.description}`);
      return 0;
    case "compare": {
      const opts = { thresholdPercent: args.thresholdPercent, minDeltaMs: args.minDeltaMs };
      const rows = compareResults(readResult(args.before), readResult(args.after), opts);
      console.log(renderCompare(rows, opts));
      return hasRegression(rows) ? 1 : 0;
    }
    case "run": {
      // Check output paths before a multi-minute run, not after it.
      for (const out of [args.jsonOut, args.markdownOut]) {
        if (out !== null && !existsSync(dirname(out))) throw new Error(`output directory does not exist: ${dirname(out)}`);
      }
      let pending: (() => Promise<void>) | undefined;
      for (const sig of ["SIGINT", "SIGTERM"] as const) {
        process.on(sig, () => void (pending?.() ?? Promise.resolve()).finally(() => process.exit(130)));
      }
      const result = await runBenchmarks(args, (cleanup) => (pending = cleanup));
      console.log(`${renderEnvironment(result)}\n\n${renderTable(result)}`);
      if (args.jsonOut) writeFileSync(args.jsonOut, `${JSON.stringify(result, null, 2)}\n`);
      if (args.markdownOut) writeFileSync(args.markdownOut, renderMarkdown(result));
      return 0;
    }
  }
}

try {
  process.exitCode = await main(parseCli(process.argv.slice(2), process.env));
} catch (error) {
  console.error(`error: ${error instanceof Error ? error.message : String(error)}\n\n${USAGE}`);
  process.exitCode = 2;
}

/** Command-line parsing and validation. Pure given argv and env. */

import { parseArgs } from "node:util";

export interface RunArgs {
  command: "run";
  baseUrl: string;
  authToken: string | undefined;
  iterations: number;
  pauseMs: number;
  concurrency: number;
  execSamples: number;
  only: string[];
  hardware: string | null;
  jsonOut: string | null;
  markdownOut: string | null;
  allowBusy: boolean;
  imageId: string | null;
}

export interface CompareArgs {
  command: "compare";
  before: string;
  after: string;
  thresholdPercent: number;
  minDeltaMs: number;
}

export type Args = RunArgs | CompareArgs | { command: "list" } | { command: "help" };

export const USAGE = `Usage:
  node src/cli.ts [run] [options]        run the benchmarks
  node src/cli.ts compare BEFORE.json AFTER.json [--threshold 20] [--min-delta-ms 0.5]
  node src/cli.ts --list                 list benchmarks

Run options:
  --iterations N     rounds per benchmark (default 20)
  --pause-ms N       wait N ms between iterations of create/snapshot, so a warm pool can refill (default 0: back to back)
  --concurrency C    simultaneous creates in the parallel benchmark (default 8)
  --exec-samples N   exec round trips on one open sandbox (default 200)
  --only a,b         run only these benchmarks (see --list)
  --base-url URL     daemon URL (env SANDKILN_DAEMON_URL, default http://127.0.0.1:7777)
  --token TOKEN      auth token (env SANDKILN_AUTH_TOKEN)
  --hardware LABEL   free-form hardware description stored in the result
  --json FILE        write the diffable JSON result
  --markdown FILE    write a Markdown report
  --image ID         boot every sandbox from this registered image (kiln image create) instead of the default
  --allow-busy       run even if the daemon already has live sandboxes

compare exits 1 when any metric regressed beyond the threshold.`;

function nonNegativeInt(name: string, raw: string): number {
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 0) throw new Error(`--${name} must be a whole number of at least 0, got "${raw}"`);
  return n;
}

function positiveInt(name: string, raw: string): number {
  const n = Number(raw);
  if (!Number.isInteger(n) || n < 1) throw new Error(`--${name} must be a positive integer, got "${raw}"`);
  return n;
}

function nonNegativeNumber(name: string, raw: string): number {
  const n = Number(raw);
  if (!Number.isFinite(n) || n < 0) throw new Error(`--${name} must be a non-negative number, got "${raw}"`);
  return n;
}

export function parseCli(argv: readonly string[], env: Record<string, string | undefined>): Args {
  const { values, positionals } = parseArgs({
    args: [...argv],
    allowPositionals: true,
    options: {
      iterations: { type: "string" },
      "pause-ms": { type: "string" },
      concurrency: { type: "string" },
      "exec-samples": { type: "string" },
      only: { type: "string" },
      "base-url": { type: "string" },
      token: { type: "string" },
      hardware: { type: "string" },
      json: { type: "string" },
      markdown: { type: "string" },
      image: { type: "string" },
      "allow-busy": { type: "boolean" },
      threshold: { type: "string" },
      "min-delta-ms": { type: "string" },
      list: { type: "boolean" },
      help: { type: "boolean", short: "h" },
    },
  });

  if (values.help) return { command: "help" };
  if (values.list) return { command: "list" };

  const [first, ...rest] = positionals;
  if (first === "compare") {
    const [before, after] = rest;
    if (before === undefined || after === undefined || rest.length > 2) throw new Error("compare needs exactly two files: BEFORE.json AFTER.json");
    return {
      command: "compare",
      before,
      after,
      thresholdPercent: nonNegativeNumber("threshold", values.threshold ?? "20"),
      minDeltaMs: nonNegativeNumber("min-delta-ms", values["min-delta-ms"] ?? "0.5"),
    };
  }
  if (first !== undefined && first !== "run") throw new Error(`unknown command "${first}"`);
  if (rest.length > 0) throw new Error(`unexpected argument "${rest[0]}"`);

  return {
    command: "run",
    baseUrl: values["base-url"] ?? env.SANDKILN_DAEMON_URL ?? "http://127.0.0.1:7777",
    authToken: values.token ?? env.SANDKILN_AUTH_TOKEN,
    iterations: positiveInt("iterations", values.iterations ?? "20"),
    pauseMs: nonNegativeInt("pause-ms", values["pause-ms"] ?? "0"),
    concurrency: positiveInt("concurrency", values.concurrency ?? "8"),
    execSamples: positiveInt("exec-samples", values["exec-samples"] ?? "200"),
    only: values.only?.split(",").map((s) => s.trim()).filter(Boolean) ?? [],
    hardware: values.hardware ?? null,
    jsonOut: values.json ?? null,
    markdownOut: values.markdown ?? null,
    imageId: values.image === undefined || values.image === "" ? null : values.image,
    allowBusy: values["allow-busy"] ?? false,
  };
}

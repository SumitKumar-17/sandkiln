/** Level of one doctor check: `warn` is worth acting on, `fail` means the daemon is not usable. */
export type CheckLevel = "ok" | "warn" | "fail";

export interface CheckResult {
  level: CheckLevel;
  detail: string;
}

export interface Check {
  name: string;
  /** A failing check with `blocking` set stops the run: later checks cannot succeed without it. */
  blocking?: boolean;
  run: () => Promise<CheckResult>;
}

export interface CheckOutcome extends CheckResult {
  name: string;
  ms: number;
}

/** Runs checks in order, timing each; a thrown error becomes a `fail`; stops after a blocking failure. */
export async function runChecks(checks: Check[], now: () => number = Date.now): Promise<CheckOutcome[]> {
  const outcomes: CheckOutcome[] = [];
  for (const check of checks) {
    const started = now();
    let result: CheckResult;
    try {
      result = await check.run();
    } catch (error) {
      result = { level: "fail", detail: error instanceof Error ? error.message : String(error) };
    }
    outcomes.push({ name: check.name, ms: now() - started, ...result });
    if (result.level === "fail" && check.blocking) {
      break;
    }
  }
  return outcomes;
}

const MARK: Record<CheckLevel, string> = { ok: "ok  ", warn: "warn", fail: "FAIL" };

export function formatOutcomes(outcomes: CheckOutcome[]): string {
  const width = Math.max(...outcomes.map((o) => o.name.length), 0);
  return outcomes.map((o) => `${MARK[o.level]}  ${o.name.padEnd(width)}  ${o.detail} (${o.ms}ms)`).join("\n") + "\n";
}

/** 0 healthy, 1 any failure, 2 warnings only: matches `kiln status`. */
export function exitCodeFor(outcomes: CheckOutcome[]): number {
  if (outcomes.some((o) => o.level === "fail")) return 1;
  return outcomes.some((o) => o.level === "warn") ? 2 : 0;
}

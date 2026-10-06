import type { Command } from "commander";
import { SandkilnApiError } from "sandkiln";

export interface GlobalOptions {
  baseUrl?: string;
  token?: string;
}

export function clientOptions(cmd: Command): GlobalOptions {
  return cmd.optsWithGlobals();
}

export async function fail(message: string): Promise<never> {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}

export async function handleApiError(error: unknown): Promise<never> {
  if (error instanceof SandkilnApiError) {
    return fail(`error: ${error.message} (status ${error.status})`);
  }
  return fail(`error: ${error instanceof Error ? error.message : String(error)}`);
}

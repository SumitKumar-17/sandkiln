/** Server settings, parsed from the environment. Pure so it is unit-testable. */

export interface McpConfig {
  /** Daemon URL; undefined lets the SDK fall back to its own default. */
  baseUrl: string | undefined;
  /** Bearer token; never logged or echoed in any tool result. */
  authToken: string | undefined;
  /** Cap on text returned to the model by any single tool call. */
  maxOutputBytes: number;
  /** Most sandboxes this session may have live at once. */
  maxSandboxes: number;
  /** When true, sandboxes still live at exit are stopped with `keep=true` (snapshotted) instead of destroyed. */
  keepOnExit: boolean;
}

export const DEFAULT_MAX_OUTPUT_BYTES = 32 * 1024;
export const DEFAULT_MAX_SANDBOXES = 8;

export class ConfigError extends Error {}

function positiveInt(env: NodeJS.ProcessEnv, key: string, fallback: number): number {
  const raw = env[key];
  if (raw === undefined || raw === "") return fallback;
  const value = Number(raw);
  if (!Number.isInteger(value) || value < 1) {
    throw new ConfigError(`${key} must be a positive integer, got "${raw}"`);
  }
  return value;
}

function flag(env: NodeJS.ProcessEnv, key: string): boolean {
  const raw = env[key]?.toLowerCase();
  if (raw === undefined || raw === "" || raw === "0" || raw === "false") return false;
  if (raw === "1" || raw === "true") return true;
  throw new ConfigError(`${key} must be 1/true or 0/false, got "${env[key]}"`);
}

export function parseConfig(env: NodeJS.ProcessEnv): McpConfig {
  return {
    baseUrl: env.SANDKILN_DAEMON_URL || undefined,
    authToken: env.SANDKILN_AUTH_TOKEN || undefined,
    maxOutputBytes: positiveInt(env, "SANDKILN_MCP_MAX_OUTPUT_BYTES", DEFAULT_MAX_OUTPUT_BYTES),
    maxSandboxes: positiveInt(env, "SANDKILN_MCP_MAX_SANDBOXES", DEFAULT_MAX_SANDBOXES),
    keepOnExit: flag(env, "SANDKILN_MCP_KEEP_ON_EXIT"),
  };
}

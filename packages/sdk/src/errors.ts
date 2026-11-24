import type { ExecTimeoutResponseBody } from "./types.js";

export class SandkilnApiError extends Error {
  readonly status: number;
  /** The parsed JSON error body when the daemon sent one. */
  readonly body: unknown;

  constructor(status: number, message: string, body?: unknown) {
    super(message);
    this.name = "SandkilnApiError";
    this.status = status;
    this.body = body;
  }
}

/**
 * `runCommand` hit its `timeoutSeconds`: the guest killed the command's whole
 * process group, and this carries whatever it had printed by then. A subclass
 * of `SandkilnApiError` (status 504), so code that already catches that keeps
 * working.
 */
export class SandkilnExecTimeoutError extends SandkilnApiError {
  readonly stdout: string;
  readonly stderr: string;
  readonly timeoutSeconds: number;

  constructor(message: string, fields: { stdout: string; stderr: string; timeoutSeconds: number }, body?: unknown) {
    super(504, message, body);
    this.name = "SandkilnExecTimeoutError";
    this.stdout = fields.stdout;
    this.stderr = fields.stderr;
    this.timeoutSeconds = fields.timeoutSeconds;
  }
}

/** Turns the daemon's "command killed at its limit" 504 into a typed error;
 * any other error (including a bare 504 from a reply that never came) is not
 * one and yields `undefined`. */
export function asExecTimeout(err: unknown): SandkilnExecTimeoutError | undefined {
  if (!(err instanceof SandkilnApiError) || err.status !== 504) return undefined;
  const body = err.body as Partial<ExecTimeoutResponseBody> | undefined;
  if (body?.timed_out !== true) return undefined;
  return new SandkilnExecTimeoutError(
    err.message,
    { stdout: body.stdout ?? "", stderr: body.stderr ?? "", timeoutSeconds: body.timeout_secs ?? 0 },
    err.body,
  );
}

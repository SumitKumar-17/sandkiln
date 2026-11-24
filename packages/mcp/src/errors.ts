import { SandkilnApiError } from "sandkiln";

/** What a tool reports when it fails. Models act on `kind` and `hint`, so
 * these stay stable and short; `message` carries the daemon's own text. */
export interface ToolError {
  kind:
    | "invalid_request"
    | "unauthorized"
    | "not_found"
    | "conflict"
    | "rate_limited"
    | "daemon_error"
    | "daemon_unreachable"
    | "timeout"
    | "limit_reached"
    | "internal";
  message: string;
  status?: number;
  hint?: string;
  /** Extra structured fields for kinds that carry data, such as the partial output of a timed-out command. */
  details?: Record<string, unknown>;
}

/** Raised by this package's own guards (timeouts, session limits). */
export class ToolFailure extends Error {
  constructor(
    readonly kind: ToolError["kind"],
    message: string,
    readonly hint?: string,
    readonly details?: Record<string, unknown>,
  ) {
    super(message);
  }
}

function fromStatus(status: number): { kind: ToolError["kind"]; hint?: string } {
  if (status === 400 || status === 422) return { kind: "invalid_request", hint: "Check the arguments against the tool schema." };
  if (status === 401 || status === 403) return { kind: "unauthorized", hint: "The daemon rejected the credentials: check SANDKILN_AUTH_TOKEN." };
  if (status === 404) return { kind: "not_found", hint: "The id does not exist. It may have been stopped or snapshotted; call sandbox_list or snapshot_list." };
  if (status === 409) return { kind: "conflict", hint: "The resource is in a conflicting state, for example a snapshot that already has a live fork, or a name already taken." };
  if (status === 429) return { kind: "rate_limited", hint: "The daemon is at capacity. Wait and retry, or stop sandboxes you no longer need." };
  return { kind: "daemon_error" };
}

export function toToolError(err: unknown, baseUrl: string | undefined): ToolError {
  if (err instanceof ToolFailure) {
    return {
      kind: err.kind,
      message: err.message,
      ...(err.hint ? { hint: err.hint } : {}),
      ...(err.details ? { details: err.details } : {}),
    };
  }
  if (err instanceof SandkilnApiError) {
    const { kind, hint } = fromStatus(err.status);
    return { kind, message: err.message, status: err.status, ...(hint ? { hint } : {}) };
  }
  // fetch() rejects with a TypeError when the daemon cannot be reached at all.
  if (err instanceof TypeError && /fetch failed|network|ECONNREFUSED/i.test(`${err.message} ${String(err.cause ?? "")}`)) {
    return {
      kind: "daemon_unreachable",
      message: `could not reach the sandkiln daemon at ${baseUrl ?? "the default address"}`,
      hint: "Check that sandkilnd is running and SANDKILN_DAEMON_URL points at it.",
    };
  }
  return { kind: "internal", message: err instanceof Error ? err.message : String(err) };
}

import { SandkilnApiError } from "./errors.js";
import type { ApiErrorBody } from "./types.js";

export interface RequestOptions {
  baseUrl: string;
  method: "GET" | "POST" | "DELETE";
  path: string;
  body?: unknown;
  authToken?: string;
}

export async function request<T>(options: RequestOptions): Promise<T> {
  const headers: Record<string, string> = {};
  if (options.body !== undefined) {
    headers["content-type"] = "application/json";
  }
  if (options.authToken !== undefined) {
    headers.authorization = `Bearer ${options.authToken}`;
  }

  const response = await fetch(`${options.baseUrl}${options.path}`, {
    method: options.method,
    headers: Object.keys(headers).length > 0 ? headers : undefined,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
  });

  if (!response.ok) {
    throw new SandkilnApiError(response.status, await extractErrorMessage(response));
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return (await response.json()) as T;
}

async function extractErrorMessage(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as ApiErrorBody;
    if (typeof body.error === "string") {
      return body.error;
    }
  } catch {
    // Non-JSON error body (e.g. a proxy in front of the daemon); fall back below.
  }
  return response.statusText || `request failed with status ${response.status}`;
}

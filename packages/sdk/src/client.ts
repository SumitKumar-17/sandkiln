import { resolveAuthToken, resolveBaseUrl } from "./config.js";

/** Resolved `baseUrl`/`authToken` pair every request-issuing SDK class
 * (`Sandbox`, `Image`) carries — pulled out of `sandbox.ts` once `Image`
 * needed the exact same resolution logic, rather than duplicating it. */
export interface ClientContext {
  baseUrl: string;
  authToken?: string;
}

export function resolveClient(options: { baseUrl?: string; authToken?: string }): ClientContext {
  return { baseUrl: resolveBaseUrl(options.baseUrl), authToken: resolveAuthToken(options.authToken) };
}

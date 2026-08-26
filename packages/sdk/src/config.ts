const DEFAULT_BASE_URL = "http://127.0.0.1:7777";

export function resolveBaseUrl(baseUrl?: string): string {
  const chosen = baseUrl ?? readEnv("SANDKILN_DAEMON_URL") ?? DEFAULT_BASE_URL;
  return chosen.endsWith("/") ? chosen.slice(0, -1) : chosen;
}

export function resolveAuthToken(authToken?: string): string | undefined {
  return authToken ?? readEnv("SANDKILN_AUTH_TOKEN");
}

// process may not exist outside Node; guard rather than assume the SDK only ever runs there.
function readEnv(key: string): string | undefined {
  return typeof process !== "undefined" ? process.env?.[key] : undefined;
}

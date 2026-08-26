const DEFAULT_BASE_URL = "http://127.0.0.1:7777";

export function resolveBaseUrl(baseUrl?: string): string {
  const chosen = baseUrl ?? envBaseUrl() ?? DEFAULT_BASE_URL;
  return chosen.endsWith("/") ? chosen.slice(0, -1) : chosen;
}

// process may not exist outside Node; guard rather than assume the SDK only ever runs there.
function envBaseUrl(): string | undefined {
  return typeof process !== "undefined" ? process.env?.SANDKILN_DAEMON_URL : undefined;
}

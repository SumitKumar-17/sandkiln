import type { SandboxInfo } from "sandkiln";

export function parseTag(value: string, previous: Record<string, string>): Record<string, string> {
  const separatorIndex = value.indexOf("=");
  if (separatorIndex === -1) {
    throw new Error(`--tag expects key=value, got: ${value}`);
  }
  previous[value.slice(0, separatorIndex)] = value.slice(separatorIndex + 1);
  return previous;
}

function formatSandboxLine(info: SandboxInfo): string {
  const tags = Object.entries(info.tags)
    .map(([k, v]) => `${k}=${v}`)
    .join(",");
  return `${info.id}  ${info.createdAt.toISOString()}  ${tags}`;
}

export function formatSandboxList(sandboxes: SandboxInfo[]): string {
  if (sandboxes.length === 0) {
    return "no sandboxes\n";
  }
  return sandboxes.map(formatSandboxLine).join("\n") + "\n";
}

import { InvalidArgumentError } from "commander";
import type { ImageInfo, SandboxInfo, SnapshotInfo } from "sandkiln";

/**
 * A plain `Error` here crashes with a raw stack trace instead of the
 * clean `error: ...` message every other failure in this CLI produces —
 * commander only turns a thrown error into that clean message when it's
 * tagged `commander.invalidArgument`, which is what this class does.
 */
export function parseTag(value: string, previous: Record<string, string>): Record<string, string> {
  const separatorIndex = value.indexOf("=");
  if (separatorIndex === -1) {
    throw new InvalidArgumentError(`--tag expects key=value, got: ${value}`);
  }
  previous[value.slice(0, separatorIndex)] = value.slice(separatorIndex + 1);
  return previous;
}

/**
 * Returns a commander argument parser for a flag that must be a positive
 * whole number (`--vcpu`, `--mem`) — `flag` is only used to name the
 * flag in the error message on invalid input, so one bad `Number(...)`
 * (`NaN`, `0`, a fraction) fails the command loudly instead of silently
 * turning into "no override" once it reaches the daemon.
 */
export function parsePositiveInt(flag: string): (value: string) => number {
  return (value: string): number => {
    const parsed = Number(value);
    if (!Number.isInteger(parsed) || parsed <= 0) {
      throw new InvalidArgumentError(`${flag} expects a positive whole number, got: ${value}`);
    }
    return parsed;
  };
}

function formatSandboxLine(info: SandboxInfo): string {
  const tags = Object.entries(info.tags)
    .map(([k, v]) => `${k}=${v}`)
    .join(",");
  return `${info.id}  ${info.createdAt.toISOString()}  ${info.name ?? "-"}  ${tags}`;
}

export function formatSandboxList(sandboxes: SandboxInfo[]): string {
  if (sandboxes.length === 0) {
    return "no sandboxes\n";
  }
  return sandboxes.map(formatSandboxLine).join("\n") + "\n";
}

function formatSnapshotLine(info: SnapshotInfo): string {
  const tags = Object.entries(info.tags)
    .map(([k, v]) => `${k}=${v}`)
    .join(",");
  const forked = info.forkedInto !== null ? `  forked_into=${info.forkedInto}` : "";
  return `${info.id}  source=${info.sourceSandboxId}  ${info.createdAt.toISOString()}  ${tags}${forked}`;
}

export function formatSnapshotList(snapshots: SnapshotInfo[]): string {
  if (snapshots.length === 0) {
    return "no snapshots\n";
  }
  return snapshots.map(formatSnapshotLine).join("\n") + "\n";
}

function formatImageLine(info: ImageInfo): string {
  const inUseBy = info.inUseBy ?? "not in use";
  return `${info.id}  ${info.sizeMib}MiB  ${info.createdAt.toISOString()}  ${inUseBy}`;
}

export function formatImageList(images: ImageInfo[]): string {
  if (images.length === 0) {
    return "no images\n";
  }
  return images.map(formatImageLine).join("\n") + "\n";
}

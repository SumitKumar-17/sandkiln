import { InvalidArgumentError } from "commander";
import type { DirEntry, DriveInfo, ImageInfo, SandboxInfo, SnapshotInfo } from "sandkiln";

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
 * Parses a repeatable `--drive <id>` / `--drive <id>:ro` flag into the
 * `{ id, readOnly }` shape `Sandbox.create`'s `drives` option expects —
 * `:ro` is the only recognized suffix (read-only); anything else after
 * the id is rejected rather than silently ignored.
 */
export function parseDriveAttachment(
  value: string,
  previous: { id: string; readOnly?: boolean }[],
): { id: string; readOnly?: boolean }[] {
  const separatorIndex = value.indexOf(":");
  if (separatorIndex === -1) {
    previous.push({ id: value });
    return previous;
  }
  const id = value.slice(0, separatorIndex);
  const suffix = value.slice(separatorIndex + 1);
  if (suffix !== "ro") {
    throw new InvalidArgumentError(`--drive expects <id> or <id>:ro, got: ${value}`);
  }
  previous.push({ id, readOnly: true });
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

/**
 * Returns a commander argument parser for `chmod`'s `<mode>` — always
 * parsed as octal digits with no `0o`/`0` prefix required, matching the
 * `chmod` shell command's own convention (`chmod 644 file`, not
 * `chmod 0o644 file`).
 */
export function parseOctalMode(value: string): number {
  if (!/^[0-7]{1,4}$/.test(value)) {
    throw new InvalidArgumentError(`<mode> expects 1-4 octal digits (e.g. 644), got: ${value}`);
  }
  return parseInt(value, 8);
}

/**
 * Like `parsePositiveInt`, but allows `0` — for flags where zero is a
 * meaningful value (e.g. `truncate`'s `<size>`, unlike `--vcpu`/`--mem`
 * where `0` is always meaningless).
 */
export function parseNonNegativeInt(flag: string): (value: string) => number {
  return (value: string): number => {
    const parsed = Number(value);
    if (!Number.isInteger(parsed) || parsed < 0) {
      throw new InvalidArgumentError(`${flag} expects a non-negative whole number, got: ${value}`);
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

function formatDriveLine(info: DriveInfo): string {
  const holders =
    info.attachedTo.length === 0
      ? "not attached"
      : info.attachedTo.map((h) => `${h.holder}${h.readOnly ? ":ro" : ""}`).join(",");
  return `${info.id}  ${info.sizeMib}MiB  ${info.createdAt.toISOString()}  ${holders}`;
}

export function formatDriveList(drives: DriveInfo[]): string {
  if (drives.length === 0) {
    return "no drives\n";
  }
  return drives.map(formatDriveLine).join("\n") + "\n";
}

function formatDirEntryLine(entry: DirEntry): string {
  const kind = entry.isDir ? "d" : entry.isSymlink ? "l" : "-";
  const mode = entry.mode.toString(8).padStart(3, "0");
  return `${kind}${mode}  ${entry.size}  ${entry.mtime.toISOString()}  ${entry.name}`;
}

export function formatDirEntryList(entries: DirEntry[]): string {
  if (entries.length === 0) {
    return "empty directory\n";
  }
  return entries.map(formatDirEntryLine).join("\n") + "\n";
}

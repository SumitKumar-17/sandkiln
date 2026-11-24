import { chmod, lstat, mkdir, readFile, readdir, readlink, symlink, writeFile } from "node:fs/promises";
import path from "node:path";
import type { DirEntry } from "./types.js";

/** The slice of `Sandbox` the transfer helpers use, so they can be tested with an in-memory fake. */
export interface TransferTarget {
  readFile(path: string): Promise<Uint8Array>;
  writeFile(path: string, content: string | Uint8Array): Promise<void>;
  mkdir(path: string, options?: { parents?: boolean }): Promise<void>;
  chmod(path: string, mode: number): Promise<void>;
  symlink(target: string, linkPath: string): Promise<void>;
  readlink(path: string): Promise<string>;
  listDir(path: string): Promise<DirEntry[]>;
}

export interface TransferOptions {
  /** Files in flight at once. Default 8. */
  concurrency?: number;
  /** Skip an entry when this returns true; receives the path relative to the root, `/`-separated. */
  exclude?: (relativePath: string) => boolean;
}

export interface TransferResult {
  files: number;
  directories: number;
  symlinks: number;
  bytes: number;
  /** Entries left out (excluded, or a symlink that would escape the destination). */
  skipped: string[];
}

const DEFAULT_CONCURRENCY = 8;

/**
 * Joins an entry name from an untrusted source (a guest's directory
 * listing) under `root`, refusing anything that is not a single plain
 * path component. This is what keeps a hostile guest from writing outside
 * the download destination.
 */
export function safeChild(root: string, name: string): string {
  if (name === "" || name === "." || name === ".." || name.includes("/") || name.includes("\\") || name.includes("\0")) {
    throw new Error(`refusing unsafe entry name ${JSON.stringify(name)}`);
  }
  return path.join(root, name);
}

/** True if a symlink created at `linkDir/<name>` pointing to `target` stays inside `root`. */
export function symlinkStaysInside(root: string, linkDir: string, target: string): boolean {
  if (path.isAbsolute(target)) return false;
  const resolved = path.resolve(linkDir, target);
  const rel = path.relative(root, resolved);
  return rel === "" || (!rel.startsWith("..") && !path.isAbsolute(rel));
}

async function runLimited(tasks: Array<() => Promise<void>>, concurrency: number): Promise<void> {
  let next = 0;
  const worker = async () => {
    while (next < tasks.length) {
      const task = tasks[next++]!;
      await task();
    }
  };
  await Promise.all(Array.from({ length: Math.min(concurrency, tasks.length) }, worker));
}

function posixJoin(dir: string, name: string): string {
  return dir.endsWith("/") ? dir + name : `${dir}/${name}`;
}

/** Copies a local directory tree into the sandbox, keeping file modes and symlinks. */
export async function uploadDir(
  target: TransferTarget,
  localDir: string,
  remoteDir: string,
  options: TransferOptions = {},
): Promise<TransferResult> {
  const result: TransferResult = { files: 0, directories: 0, symlinks: 0, bytes: 0, skipped: [] };
  const fileTasks: Array<() => Promise<void>> = [];
  const symlinks: Array<[string, string]> = [];

  await target.mkdir(remoteDir, { parents: true });
  result.directories++;

  const walk = async (local: string, remote: string, relative: string): Promise<void> => {
    for (const entry of await readdir(local, { withFileTypes: true })) {
      const rel = relative === "" ? entry.name : `${relative}/${entry.name}`;
      if (options.exclude?.(rel)) {
        result.skipped.push(rel);
        continue;
      }
      const localPath = path.join(local, entry.name);
      const remotePath = posixJoin(remote, entry.name);
      if (entry.isDirectory()) {
        await target.mkdir(remotePath, { parents: true });
        result.directories++;
        await walk(localPath, remotePath, rel);
      } else if (entry.isSymbolicLink()) {
        symlinks.push([await readlink(localPath), remotePath]);
      } else if (entry.isFile()) {
        fileTasks.push(async () => {
          const content = await readFile(localPath);
          await target.writeFile(remotePath, content);
          await target.chmod(remotePath, (await lstat(localPath)).mode & 0o7777);
          result.files++;
          result.bytes += content.length;
        });
      } else {
        result.skipped.push(rel);
      }
    }
  };
  await walk(localDir, remoteDir, "");
  await runLimited(fileTasks, options.concurrency ?? DEFAULT_CONCURRENCY);
  for (const [linkTarget, linkPath] of symlinks) {
    await target.symlink(linkTarget, linkPath);
    result.symlinks++;
  }
  return result;
}

/**
 * Copies a directory tree out of the sandbox. The guest is untrusted, so
 * entry names are validated (`safeChild`) and a symlink is only recreated
 * when its target stays inside the destination; others are reported in
 * `skipped` rather than written.
 */
export async function downloadDir(
  target: TransferTarget,
  remoteDir: string,
  localDir: string,
  options: TransferOptions = {},
): Promise<TransferResult> {
  const result: TransferResult = { files: 0, directories: 0, symlinks: 0, bytes: 0, skipped: [] };
  const fileTasks: Array<() => Promise<void>> = [];
  const root = path.resolve(localDir);
  await mkdir(root, { recursive: true });
  result.directories++;

  const walk = async (remote: string, local: string, relative: string): Promise<void> => {
    for (const entry of await target.listDir(remote)) {
      const localPath = safeChild(local, entry.name);
      const rel = relative === "" ? entry.name : `${relative}/${entry.name}`;
      if (options.exclude?.(rel)) {
        result.skipped.push(rel);
        continue;
      }
      const remotePath = posixJoin(remote, entry.name);
      if (entry.isSymlink) {
        const linkTarget = await target.readlink(remotePath);
        if (symlinkStaysInside(root, local, linkTarget)) {
          await symlink(linkTarget, localPath);
          result.symlinks++;
        } else {
          result.skipped.push(rel);
        }
      } else if (entry.isDir) {
        await mkdir(localPath, { recursive: true });
        result.directories++;
        await walk(remotePath, localPath, rel);
      } else {
        fileTasks.push(async () => {
          const content = await target.readFile(remotePath);
          await writeFile(localPath, content);
          await chmod(localPath, entry.mode & 0o777);
          result.files++;
          result.bytes += content.length;
        });
      }
    }
  };
  await walk(remoteDir, root, "");
  await runLimited(fileTasks, options.concurrency ?? DEFAULT_CONCURRENCY);
  return result;
}

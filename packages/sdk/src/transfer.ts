import { constants as fsConstants } from "node:fs";
import { lstat, mkdir, open, readFile, readdir, readlink, realpath, rm, symlink } from "node:fs/promises";
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

/**
 * True if a symlink at `linkDir/<name>` pointing to `target` would resolve
 * inside `root` given what is on disk now. The target is walked one component
 * at a time through the real filesystem, because collapsing `..` lexically is
 * wrong once links exist: with `s -> ..` on disk, `s/..` leaves the root even
 * though it looks like a no-op. Every step has to stay inside; anything that
 * cannot be resolved with confidence is refused.
 */
export async function symlinkStaysInside(root: string, linkDir: string, target: string): Promise<boolean> {
  if (path.isAbsolute(target)) return false;
  const realRoot = await realpath(root);
  const inside = (p: string) => {
    const rel = path.relative(realRoot, p);
    return rel === "" || (!rel.startsWith("..") && !path.isAbsolute(rel));
  };
  let current = await realpath(linkDir);
  const segments = target.split("/").filter((seg) => seg !== "" && seg !== ".");
  for (const [i, seg] of segments.entries()) {
    if (seg === "..") {
      current = path.dirname(current);
    } else {
      const next = path.join(current, seg);
      const info = await lstat(next).catch(() => null);
      if (info === null) {
        // Does not exist yet: the rest cannot traverse a link, so it is lexical.
        const rest = segments.slice(i);
        if (rest.includes("..")) return false;
        current = path.join(current, ...rest);
        return inside(current);
      }
      current = info.isSymbolicLink() ? await realpath(next).catch(() => null) as string : next;
      if (current === null) return false;
    }
    if (!inside(current)) return false;
  }
  return inside(current);
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
  const links: Array<{ dir: string; name: string; target: string; rel: string }> = [];
  const root = path.resolve(localDir);
  await mkdir(root, { recursive: true });
  result.directories++;

  const walk = async (remote: string, local: string, relative: string): Promise<void> => {
    const seen = new Set<string>();
    for (const entry of await target.listDir(remote)) {
      // A listing naming one path twice is how a hostile guest turns "create a
      // link, then write a file of the same name" into a write through the link.
      if (seen.has(entry.name)) throw new Error(`refusing duplicate entry name ${JSON.stringify(entry.name)} in ${remote}`);
      seen.add(entry.name);
      const localPath = safeChild(local, entry.name);
      const rel = relative === "" ? entry.name : `${relative}/${entry.name}`;
      if (options.exclude?.(rel)) {
        result.skipped.push(rel);
        continue;
      }
      const remotePath = posixJoin(remote, entry.name);
      if (entry.isSymlink) {
        links.push({ dir: local, name: entry.name, target: await target.readlink(remotePath), rel });
      } else if (entry.isDir) {
        // Never descend through a symlink that is already on disk.
        const existing = await lstat(localPath).catch(() => null);
        if (existing?.isSymbolicLink()) {
          result.skipped.push(rel);
          continue;
        }
        await mkdir(localPath, { recursive: true });
        result.directories++;
        await walk(remotePath, localPath, rel);
      } else {
        const mode = entry.mode & 0o777; // drops setuid, setgid and sticky
        fileTasks.push(async () => {
          const content = await target.readFile(remotePath);
          // O_NOFOLLOW: never write through a symlink already at this path.
          const handle = await open(localPath, fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_TRUNC | fsConstants.O_NOFOLLOW, 0o600);
          try {
            await handle.writeFile(content);
            await handle.chmod(mode);
          } finally {
            await handle.close();
          }
          result.files++;
          result.bytes += content.length;
        });
      }
    }
  };
  await walk(remoteDir, root, "");
  await runLimited(fileTasks, options.concurrency ?? DEFAULT_CONCURRENCY);

  // Links last and one at a time, each checked against what is on disk by then,
  // so a chain of links is judged by where it really resolves.
  for (const link of links) {
    if (await symlinkStaysInside(root, link.dir, link.target)) {
      await symlink(link.target, path.join(link.dir, link.name));
      result.symlinks++;
    } else {
      result.skipped.push(link.rel);
    }
  }
  return result;
}

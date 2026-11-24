// File and directory operations inside a sandbox.
import { readFileSync } from "node:fs";
import { Command } from "commander";
import { formatDirEntryList, parseNonNegativeInt, parseOctalMode, parsePositiveInt } from "../../format.js";
import { clientOptions, handleApiError } from "../shared.js";
import { attachSandbox } from "./helpers.js";

export function registerRead(sandbox: Command): void {
  sandbox
    .command("read <id> <path>")
    .description("Read a file from a sandbox and print it to stdout.")
    .action(async function (this: Command, id: string, path: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const bytes = await attachSandbox(id, baseUrl, token).readFile(path);
        process.stdout.write(Buffer.from(bytes));
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerWrite(sandbox: Command): void {
  sandbox
    .command("write <id> <path> <local-file>")
    .description("Write a local file into a sandbox at the given path.")
    .action(async function (this: Command, id: string, path: string, localFile: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const content = readFileSync(localFile);
        await attachSandbox(id, baseUrl, token).writeFile(path, content);
        process.stdout.write(`wrote ${localFile} -> ${id}:${path}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerChmod(sandbox: Command): void {
  sandbox
    .command("chmod <id> <path> <mode>")
    .description("Change a file's permission bits. <mode> is octal digits (e.g. 644), same as the chmod shell command.")
    .action(async function (this: Command, id: string, path: string, mode: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        await attachSandbox(id, baseUrl, token).chmod(path, parseOctalMode(mode));
        process.stdout.write(`chmod ${mode} ${id}:${path}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerChown(sandbox: Command): void {
  sandbox
    .command("chown <id> <path> <uid> <gid>")
    .description("Change a file's owning uid/gid.")
    .action(async function (this: Command, id: string, path: string, uid: string, gid: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        await attachSandbox(id, baseUrl, token).chown(path, parsePositiveInt("<uid>")(uid), parsePositiveInt("<gid>")(gid));
        process.stdout.write(`chown ${uid}:${gid} ${id}:${path}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerMkdir(sandbox: Command): void {
  sandbox
    .command("mkdir <id> <path>")
    .description("Create a directory inside a sandbox.")
    .option("-p, --parents", "create missing parent directories, like mkdir -p; succeed if the target already exists")
    .action(async function (this: Command, id: string, path: string, opts: { parents?: boolean }) {
      const { baseUrl, token } = clientOptions(this);
      try {
        await attachSandbox(id, baseUrl, token).mkdir(path, { parents: opts.parents });
        process.stdout.write(`${id}:${path}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerRename(sandbox: Command): void {
  sandbox
    .command("rename <id> <from> <to>")
    .description("Rename/move a file or directory inside a sandbox.")
    .action(async function (this: Command, id: string, from: string, to: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        await attachSandbox(id, baseUrl, token).rename(from, to);
        process.stdout.write(`${id}:${from} -> ${id}:${to}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerCp(sandbox: Command): void {
  sandbox
    .command("cp <id> <from> <to>")
    .description("Copy a file to a new path inside a sandbox, leaving the original in place.")
    .action(async function (this: Command, id: string, from: string, to: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        await attachSandbox(id, baseUrl, token).copy(from, to);
        process.stdout.write(`${id}:${from} -> ${id}:${to}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerUploadDir(sandbox: Command): void {
  sandbox
    .command("upload-dir <id> <local-dir> <remote-dir>")
    .description("Copy a local directory tree into a sandbox, keeping file modes and symlinks.")
    .option("--exclude <name...>", "skip any entry with this name (for example node_modules .git)")
    .action(async function (this: Command, id: string, localDir: string, remoteDir: string, options: { exclude?: string[] }) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const names = new Set(options.exclude ?? []);
        const result = await attachSandbox(id, baseUrl, token).uploadDir(localDir, remoteDir, {
          exclude: (rel) => rel.split("/").some((part) => names.has(part)),
        });
        process.stdout.write(`${result.files} files, ${result.directories} dirs, ${result.symlinks} symlinks, ${result.bytes} bytes -> ${id}:${remoteDir}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerDownloadDir(sandbox: Command): void {
  sandbox
    .command("download-dir <id> <remote-dir> <local-dir>")
    .description("Copy a directory tree out of a sandbox. Symlinks that would point outside <local-dir> are skipped and reported.")
    .action(async function (this: Command, id: string, remoteDir: string, localDir: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const result = await attachSandbox(id, baseUrl, token).downloadDir(remoteDir, localDir);
        process.stdout.write(`${result.files} files, ${result.directories} dirs, ${result.symlinks} symlinks, ${result.bytes} bytes -> ${localDir}\n`);
        for (const skipped of result.skipped) {
          process.stderr.write(`skipped: ${skipped}\n`);
        }
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerSymlink(sandbox: Command): void {
  sandbox
    .command("symlink <id> <target> <link-path>")
    .description("Create a symlink at <link-path> pointing at <target>, both inside the sandbox.")
    .action(async function (this: Command, id: string, target: string, linkPath: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        await attachSandbox(id, baseUrl, token).symlink(target, linkPath);
        process.stdout.write(`${id}:${linkPath} -> ${target}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerReadlink(sandbox: Command): void {
  sandbox
    .command("readlink <id> <path>")
    .description("Print the target a symlink points at, exactly as stored (not resolved).")
    .action(async function (this: Command, id: string, path: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const target = await attachSandbox(id, baseUrl, token).readlink(path);
        process.stdout.write(`${target}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerTruncate(sandbox: Command): void {
  sandbox
    .command("truncate <id> <path> <size>")
    .description("Resize a file to exactly <size> bytes, padding with zeros or discarding trailing data as needed.")
    .action(async function (this: Command, id: string, path: string, size: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        await attachSandbox(id, baseUrl, token).truncate(path, parseNonNegativeInt("<size>")(size));
        process.stdout.write(`${id}:${path} -> ${size} bytes\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerLsDir(sandbox: Command): void {
  sandbox
    .command("ls-dir <id> <path>")
    .description("List a directory's contents inside a sandbox, with size/permissions/mtime.")
    .action(async function (this: Command, id: string, path: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const entries = await attachSandbox(id, baseUrl, token).listDir(path);
        process.stdout.write(formatDirEntryList(entries));
      } catch (error) {
        await handleApiError(error);
      }
    });
}

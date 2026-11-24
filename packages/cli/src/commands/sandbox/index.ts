import { Command } from "commander";
import { registerCreate, registerGetOrCreate, registerByName, registerLs, registerRm, registerSnapshot, registerSnapshots, registerResume, registerFork } from "./lifecycle.js";
import { registerExec, registerPty, registerExecStream, registerLogs } from "./exec.js";
import { registerRead, registerWrite, registerChmod, registerChown, registerMkdir, registerRename, registerCp, registerUploadDir, registerDownloadDir, registerSymlink, registerReadlink, registerTruncate, registerLsDir } from "./files.js";
import { registerTunnel, registerPreview, registerMount, registerMounts, registerUnmount } from "./network.js";

/** One registrar per subcommand, in the order `kiln sandbox --help` lists
 * them. The modules group them by concern, so the listing order is stated
 * here rather than following from the imports. */
const REGISTRARS: ((sandbox: Command) => void)[] = [
  registerCreate,
  registerGetOrCreate,
  registerByName,
  registerLs,
  registerRm,
  registerExec,
  registerRead,
  registerWrite,
  registerChmod,
  registerChown,
  registerMkdir,
  registerRename,
  registerCp,
  registerUploadDir,
  registerDownloadDir,
  registerSymlink,
  registerReadlink,
  registerTruncate,
  registerLsDir,
  registerPty,
  registerTunnel,
  registerExecStream,
  registerLogs,
  registerPreview,
  registerMount,
  registerMounts,
  registerUnmount,
  registerSnapshot,
  registerSnapshots,
  registerResume,
  registerFork,
];

export function registerSandboxCommands(program: Command): void {
  const sandbox = program.command("sandbox").description("Create, inspect, and manage sandboxes.");
  for (const register of REGISTRARS) {
    register(sandbox);
  }
}

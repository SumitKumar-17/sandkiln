import { readFileSync } from "node:fs";
import { Command, Option } from "commander";
import { Drive, Image, Pool, Sandbox, SandkilnApiError } from "sandkiln";
import {
  formatDirEntryList,
  formatDriveList,
  formatImageList,
  formatPoolList,
  formatSandboxList,
  formatSnapshotList,
  parseDriveAttachment,
  parseNonNegativeInt,
  parseOctalMode,
  parsePositiveInt,
  parseTag,
} from "./format.js";

interface GlobalOptions {
  baseUrl?: string;
  token?: string;
}

const tagOption = () => new Option("--tag <key=value>", "tag to attach (repeatable)").argParser(parseTag).default({});

function clientOptions(cmd: Command): GlobalOptions {
  return cmd.optsWithGlobals();
}

async function fail(message: string): Promise<never> {
  process.stderr.write(`${message}\n`);
  process.exit(1);
}

async function handleApiError(error: unknown): Promise<never> {
  if (error instanceof SandkilnApiError) {
    return fail(`error: ${error.message} (status ${error.status})`);
  }
  return fail(`error: ${error instanceof Error ? error.message : String(error)}`);
}

const program = new Command();
program
  .name("kiln")
  .description("Manage sandkiln sandboxes from the command line.")
  .option("--base-url <url>", "daemon URL (default: SANDKILN_DAEMON_URL or http://127.0.0.1:7777)")
  .option("--token <token>", "auth token (default: SANDKILN_AUTH_TOKEN)");

const sandbox = program.command("sandbox").description("Create, inspect, and manage sandboxes.");

sandbox
  .command("create")
  .description("Boot a new sandbox.")
  .option("--name <name>", "caller-given identity, unique among live sandboxes and held snapshots")
  .addOption(tagOption())
  .option("--vcpu <count>", "vCPU count override (daemon default if omitted)", parsePositiveInt("--vcpu"))
  .option("--mem <mib>", "memory size override in MiB (daemon default if omitted)", parsePositiveInt("--mem"))
  .option("--image <id>", "boot from a registered image instead of the daemon's default rootfs (see 'kiln image ls')")
  .option("--rate-bandwidth <bytes-per-sec>", "cap host I/O bandwidth (bytes/sec) via Firecracker's rate limiter", parsePositiveInt("--rate-bandwidth"))
  .option("--rate-ops <ops-per-sec>", "cap host I/O operations/sec via Firecracker's rate limiter", parsePositiveInt("--rate-ops"))
  .option(
    "--drive <id[:ro]>",
    "attach an existing drive (see 'kiln drive ls'); repeatable; append :ro for a read-only attachment",
    parseDriveAttachment,
    [] as { id: string; readOnly?: boolean }[],
  )
  .action(async function (
    this: Command,
    opts: {
      name?: string;
      tag: Record<string, string>;
      vcpu?: number;
      mem?: number;
      image?: string;
      rateBandwidth?: number;
      rateOps?: number;
      drive: { id: string; readOnly?: boolean }[];
    },
  ) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const created = await Sandbox.create({
        baseUrl,
        authToken: token,
        name: opts.name,
        tags: opts.tag,
        vcpuCount: opts.vcpu,
        memSizeMib: opts.mem,
        imageId: opts.image,
        rateLimit:
          opts.rateBandwidth === undefined && opts.rateOps === undefined
            ? undefined
            : { bandwidthBytesPerSec: opts.rateBandwidth, opsPerSec: opts.rateOps },
        drives: opts.drive.length > 0 ? opts.drive : undefined,
      });
      process.stdout.write(`${created.id}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("get-or-create")
  .description(
    "Resolve --name to a sandbox in one call, creating it if it doesn't exist yet: a live sandbox with this " +
      "name is returned as-is, a stopped one is resumed, otherwise a fresh one is created and named. Prints the " +
      "sandbox id, plus whether it was freshly created.",
  )
  .requiredOption("--name <name>", "name to resolve or claim")
  .addOption(tagOption())
  .option("--vcpu <count>", "vCPU count override, used only if a fresh sandbox is created", parsePositiveInt("--vcpu"))
  .option("--mem <mib>", "memory size override in MiB, used only if a fresh sandbox is created", parsePositiveInt("--mem"))
  .option(
    "--rate-bandwidth <bytes-per-sec>",
    "I/O bandwidth cap (bytes/sec), used only if a fresh sandbox is created",
    parsePositiveInt("--rate-bandwidth"),
  )
  .option(
    "--rate-ops <ops-per-sec>",
    "I/O operations/sec cap, used only if a fresh sandbox is created",
    parsePositiveInt("--rate-ops"),
  )
  .option(
    "--drive <id[:ro]>",
    "attach an existing drive, used only if a fresh sandbox is created; repeatable; append :ro for read-only",
    parseDriveAttachment,
    [] as { id: string; readOnly?: boolean }[],
  )
  .action(async function (
    this: Command,
    opts: {
      name: string;
      tag: Record<string, string>;
      vcpu?: number;
      mem?: number;
      rateBandwidth?: number;
      rateOps?: number;
      drive: { id: string; readOnly?: boolean }[];
    },
  ) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const { sandbox: resolved, created } = await Sandbox.getOrCreate({
        baseUrl,
        authToken: token,
        name: opts.name,
        tags: opts.tag,
        vcpuCount: opts.vcpu,
        memSizeMib: opts.mem,
        rateLimit:
          opts.rateBandwidth === undefined && opts.rateOps === undefined
            ? undefined
            : { bandwidthBytesPerSec: opts.rateBandwidth, opsPerSec: opts.rateOps },
        drives: opts.drive.length > 0 ? opts.drive : undefined,
      });
      process.stdout.write(`${resolved.id}  ${created ? "created" : "existing"}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("by-name <name>")
  .description("Resolve a name to a live sandbox's id.")
  .action(async function (this: Command, name: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const resolved = await Sandbox.byName(name, { baseUrl, authToken: token });
      process.stdout.write(`${resolved.id}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("ls")
  .description("List sandboxes.")
  .addOption(tagOption())
  .action(async function (this: Command, opts: { tag: Record<string, string> }) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const sandboxes = await Sandbox.list({ baseUrl, authToken: token, tags: opts.tag });
      process.stdout.write(formatSandboxList(sandboxes));
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("rm <id>")
  .description(
    "Stop a sandbox. By default this preserves its state as a resumable snapshot (the daemon's default " +
      "'stop and come back later' behavior) rather than releasing everything outright.",
  )
  .option("--destroy", "fully destroy the sandbox instead — no snapshot, nothing left to resume")
  .action(async function (this: Command, id: string, opts: { destroy?: boolean }) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const result = await attachSandbox(id, baseUrl, token).stop({ keep: !opts.destroy });
      if (result.kept) {
        process.stdout.write(`${id} stopped and preserved as snapshot ${result.snapshotId}\n`);
      } else {
        process.stdout.write(`${id} stopped and destroyed\n`);
      }
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("exec <id> <command> [args...]")
  .description("Run a command inside a sandbox. Exits with the command's own exit code.")
  .action(async function (this: Command, id: string, command: string, args: string[]) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const result = await attachSandbox(id, baseUrl, token).runCommand(command, args);
      process.stdout.write(result.stdout);
      process.stderr.write(result.stderr);
      process.exit(result.exitCode);
    } catch (error) {
      await handleApiError(error);
    }
  });

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

sandbox
  .command("pty <id>")
  .description("Open a live, interactive shell session inside a sandbox. Exit the remote shell (or Ctrl+D) to end the session.")
  .action(async function (this: Command, id: string) {
    const { baseUrl, token } = clientOptions(this);
    const cols = process.stdout.columns || 80;
    const rows = process.stdout.rows || 24;

    let ws: WebSocket;
    try {
      ws = attachSandbox(id, baseUrl, token).pty({ cols, rows });
    } catch (error) {
      await handleApiError(error);
      return;
    }
    ws.binaryType = "arraybuffer";

    const wasRaw = process.stdin.isTTY ? process.stdin.isRaw : undefined;
    const restoreTerminal = () => {
      if (process.stdin.isTTY && wasRaw !== undefined) {
        process.stdin.setRawMode(wasRaw);
      }
      process.stdin.pause();
      process.stdin.removeAllListeners("data");
    };

    await new Promise<void>((resolve) => {
      ws.addEventListener("open", () => {
        if (process.stdin.isTTY) {
          process.stdin.setRawMode(true);
        }
        process.stdin.resume();
        // Every keystroke goes straight to the remote shell, including
        // Ctrl+C -- raw mode means Node never intercepts it as SIGINT,
        // the same way a real terminal/SSH client leaves Ctrl+C to
        // whatever's running remotely rather than killing the local
        // process.
        process.stdin.on("data", (chunk: Buffer) => ws.send(chunk));
      });

      ws.addEventListener("message", (event) => {
        const buf = event.data instanceof ArrayBuffer ? Buffer.from(event.data) : Buffer.from(String(event.data));
        process.stdout.write(buf);
      });

      const end = () => {
        restoreTerminal();
        resolve();
      };
      ws.addEventListener("close", end);
      ws.addEventListener("error", end);
    });

  });

sandbox
  .command("preview <id> <port>")
  .description("Print the URL to reach a server listening on <port> inside a sandbox, proxied through the daemon.")
  .option("--path <path>", "path within the sandbox's server to preview", "/")
  .action(async function (this: Command, id: string, port: string, opts: { path: string }) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const url = attachSandbox(id, baseUrl, token).previewUrl(Number(port), { path: opts.path });
      process.stdout.write(`${url}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("snapshot <id>")
  .description("Save a sandbox's full state to disk and stop it. Prints the resulting snapshot id.")
  .action(async function (this: Command, id: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const snapshotId = await attachSandbox(id, baseUrl, token).snapshot();
      process.stdout.write(`${snapshotId}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("snapshots")
  .description(
    "List snapshots. A sandbox can turn into one on its own (auto-suspend), not just via `kiln sandbox snapshot`" +
      " — use --source to find the snapshot a given sandbox id became.",
  )
  .option("--source <sandbox-id>", "only the snapshot (if any) taken from this original sandbox id")
  .action(async function (this: Command, opts: { source?: string }) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const snapshots = await Sandbox.listSnapshots({ baseUrl, authToken: token, sourceSandboxId: opts.source });
      process.stdout.write(formatSnapshotList(snapshots));
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("resume <snapshot-id>")
  .description("Boot a new sandbox from a snapshot, consuming it. Prints the new sandbox id.")
  .action(async function (this: Command, snapshotId: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const resumed = await Sandbox.resume(snapshotId, { baseUrl, authToken: token });
      process.stdout.write(`${resumed.id}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

sandbox
  .command("fork <snapshot-id>")
  .description(
    "Boot a new sandbox from a snapshot without consuming it, so it can be forked or resumed again later. " +
      "Only one live fork of a given snapshot may run at a time. Prints the new sandbox id.",
  )
  .action(async function (this: Command, snapshotId: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const forked = await Sandbox.fork(snapshotId, { baseUrl, authToken: token });
      process.stdout.write(`${forked.id}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

/** Every subcommand above only has a sandbox id, not an instance — this
 * reconstructs one without a round-trip, since every Sandbox method just
 * needs the id plus the same client config already used to reach it. */
function attachSandbox(id: string, baseUrl: string | undefined, token: string | undefined): Sandbox {
  return Sandbox.attach(id, { baseUrl, authToken: token });
}

const image = program.command("image").description("Register, inspect, and manage rootfs images sandboxes can boot from.");

image
  .command("create <id> <path>")
  .description(
    "Register an already-built ext4 rootfs file at <path> on the daemon's own host filesystem under <id> " +
      "(not a file upload — <path> must already exist where sandkilnd runs). Prints a warning: the daemon " +
      "cannot verify the guest agent is baked in without root access to loop-mount the file; run " +
      "'scripts/preflight-check.sh --root-checks --rootfs-image <path>' out of band first if you haven't already.",
  )
  .action(async function (this: Command, id: string, path: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const registered = await Image.register(id, path, { baseUrl, authToken: token });
      process.stdout.write(`${registered.id}\n`);
      if (!registered.guestAgentVerified) {
        process.stderr.write(`warning: ${registered.verificationHint}\n`);
      }
    } catch (error) {
      await handleApiError(error);
    }
  });

image
  .command("ls")
  .description("List registered images.")
  .action(async function (this: Command) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const images = await Image.list({ baseUrl, authToken: token });
      process.stdout.write(formatImageList(images));
    } catch (error) {
      await handleApiError(error);
    }
  });

image
  .command("rm <id>")
  .description("Delete a registered image. Refused while any sandbox, in-flight boot, or snapshot still references it.")
  .action(async function (this: Command, id: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      await Image.delete(id, { baseUrl, authToken: token });
      process.stdout.write(`${id} deleted\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

const drive = program.command("drive").description("Create, inspect, and manage persistent drives sandboxes can attach.");

drive
  .command("create <size-mib>")
  .description("Create a new empty drive of <size-mib> MiB, ready to attach via 'kiln sandbox create --drive'.")
  .action(async function (this: Command, sizeMib: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const created = await Drive.create(parsePositiveInt("<size-mib>")(sizeMib), { baseUrl, authToken: token });
      process.stdout.write(`${created.id}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

drive
  .command("ls")
  .description("List persistent drives.")
  .action(async function (this: Command) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const drives = await Drive.list({ baseUrl, authToken: token });
      process.stdout.write(formatDriveList(drives));
    } catch (error) {
      await handleApiError(error);
    }
  });

drive
  .command("rm <id>")
  .description("Delete a drive and its backing file. Refused while any sandbox or held snapshot still attaches it.")
  .action(async function (this: Command, id: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      await Drive.delete(id, { baseUrl, authToken: token });
      process.stdout.write(`${id} deleted\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

const pool = program.command("pool").description("Configure pre-warmed pools -- ready-to-resume snapshots claimed automatically by a matching 'kiln sandbox create'.");

pool
  .command("create <id>")
  .description(
    "Configure a pool under <id>. A plain 'kiln sandbox create' (no --drive, no rate limit) matching this pool's " +
      "image/resources resumes a warm snapshot automatically instead of cold-booting, once one is ready -- there's " +
      "no separate 'create from pool' command.",
  )
  .option("--image <id>", "boot warm instances from a registered image instead of the daemon's default rootfs")
  .option("--vcpu <count>", "vCPU count for warm instances (daemon default if omitted)", parsePositiveInt("--vcpu"))
  .option("--mem <mib>", "memory size in MiB for warm instances (daemon default if omitted)", parsePositiveInt("--mem"))
  .option("--warm-count <n>", "how many resumable snapshots to keep ready at once", parseNonNegativeInt("--warm-count"), 0)
  .action(async function (this: Command, id: string, options: { image?: string; vcpu?: number; mem?: number; warmCount: number }) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const created = await Pool.create(id, {
        baseUrl,
        authToken: token,
        imageId: options.image,
        vcpuCount: options.vcpu,
        memSizeMib: options.mem,
        warmCount: options.warmCount,
      });
      process.stdout.write(`${created.id}\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

pool
  .command("ls")
  .description("List configured pools.")
  .action(async function (this: Command) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const pools = await Pool.list({ baseUrl, authToken: token });
      process.stdout.write(formatPoolList(pools));
    } catch (error) {
      await handleApiError(error);
    }
  });

pool
  .command("rm <id>")
  .description("Remove a pool's configuration and destroy whatever it currently has warm.")
  .action(async function (this: Command, id: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      await Pool.delete(id, { baseUrl, authToken: token });
      process.stdout.write(`${id} deleted\n`);
    } catch (error) {
      await handleApiError(error);
    }
  });

// Every subcommand's own action handler already catches its errors; this
// is a backstop for anything that escapes one anyway (a bug in a future
// subcommand, or a rejection from commander's own dispatch) so a caller
// always gets a clean stderr message and exit code 1, never a raw stack
// trace.
program.parseAsync(process.argv).catch(async (error: unknown) => {
  await fail(`error: ${error instanceof Error ? error.message : String(error)}`);
});

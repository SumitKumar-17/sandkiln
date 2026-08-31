import { readFileSync } from "node:fs";
import { Command, Option } from "commander";
import { Sandbox, SandkilnApiError } from "sandkiln";
import { formatSandboxList, formatSnapshotList, parsePositiveInt, parseTag } from "./format.js";

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
  .addOption(tagOption())
  .option("--vcpu <count>", "vCPU count override (daemon default if omitted)", parsePositiveInt("--vcpu"))
  .option("--mem <mib>", "memory size override in MiB (daemon default if omitted)", parsePositiveInt("--mem"))
  .action(async function (this: Command, opts: { tag: Record<string, string>; vcpu?: number; mem?: number }) {
    const { baseUrl, token } = clientOptions(this);
    try {
      const created = await Sandbox.create({
        baseUrl,
        authToken: token,
        tags: opts.tag,
        vcpuCount: opts.vcpu,
        memSizeMib: opts.mem,
      });
      process.stdout.write(`${created.id}\n`);
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
  .description("Stop a sandbox and release its resources.")
  .action(async function (this: Command, id: string) {
    const { baseUrl, token } = clientOptions(this);
    try {
      await attachSandbox(id, baseUrl, token).stop();
      process.stdout.write(`${id} stopped\n`);
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

// Every subcommand's own action handler already catches its errors; this
// is a backstop for anything that escapes one anyway (a bug in a future
// subcommand, or a rejection from commander's own dispatch) so a caller
// always gets a clean stderr message and exit code 1, never a raw stack
// trace.
program.parseAsync(process.argv).catch(async (error: unknown) => {
  await fail(`error: ${error instanceof Error ? error.message : String(error)}`);
});

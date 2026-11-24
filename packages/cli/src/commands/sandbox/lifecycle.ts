// Sandbox lifecycle commands: create, resolve by name, list, stop, and the
// snapshot / resume / fork family.
import { Command, Option } from "commander";
import { Sandbox } from "sandkiln";
import { accumulate, formatSandboxList, formatSnapshotList, parseDriveAttachment, parsePositiveInt } from "../../format.js";
import { clientOptions, handleApiError } from "../shared.js";
import { attachSandbox, buildEgressOption, envOption, tagOption } from "./helpers.js";

export function registerCreate(sandbox: Command): void {
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
    .addOption(envOption("environment variable baked in for this sandbox's whole lifetime (repeatable); a later 'exec --env' with the same key overrides it for that one call"))
    .addOption(new Option("--egress-mode <allow_all|deny_all>", "outbound network policy default").choices(["allow_all", "deny_all"] as const))
    .option("--allow-cidr <cidr>", "CIDR to explicitly allow outbound (repeatable); only meaningful with --egress-mode", accumulate, [] as string[])
    .option("--deny-cidr <cidr>", "CIDR to explicitly deny outbound (repeatable); only meaningful with --egress-mode", accumulate, [] as string[])
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
        env: Record<string, string>;
        egressMode?: "allow_all" | "deny_all";
        allowCidr: string[];
        denyCidr: string[];
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
          env: Object.keys(opts.env).length > 0 ? opts.env : undefined,
          egress: buildEgressOption(opts),
        });
        process.stdout.write(`${created.id}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerGetOrCreate(sandbox: Command): void {
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
    .addOption(envOption("environment variable baked in for the whole sandbox lifetime, used only if a fresh sandbox is created (repeatable)"))
    .addOption(
      new Option("--egress-mode <allow_all|deny_all>", "outbound network policy default, used only if a fresh sandbox is created").choices([
        "allow_all",
        "deny_all",
      ] as const),
    )
    .option("--allow-cidr <cidr>", "CIDR to explicitly allow outbound (repeatable); only meaningful with --egress-mode", accumulate, [] as string[])
    .option("--deny-cidr <cidr>", "CIDR to explicitly deny outbound (repeatable); only meaningful with --egress-mode", accumulate, [] as string[])
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
        env: Record<string, string>;
        egressMode?: "allow_all" | "deny_all";
        allowCidr: string[];
        denyCidr: string[];
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
          env: Object.keys(opts.env).length > 0 ? opts.env : undefined,
          egress: buildEgressOption(opts),
        });
        process.stdout.write(`${resolved.id}  ${created ? "created" : "existing"}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerByName(sandbox: Command): void {
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
}

export function registerLs(sandbox: Command): void {
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
}

export function registerRm(sandbox: Command): void {
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
}

export function registerSnapshot(sandbox: Command): void {
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
}

export function registerSnapshots(sandbox: Command): void {
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
}

export function registerResume(sandbox: Command): void {
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
}

export function registerFork(sandbox: Command): void {
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
}

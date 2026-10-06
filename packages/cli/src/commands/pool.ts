import type { Command } from "commander";
import { Pool } from "sandkiln";
import { formatPoolList, parseNonNegativeInt, parsePositiveInt } from "../format.js";
import { clientOptions, handleApiError } from "./shared.js";

export function registerPoolCommands(program: Command): void {
  const pool = program
    .command("pool")
    .description("Configure pre-warmed pools -- ready-to-resume snapshots claimed automatically by a matching 'kiln sandbox create'.");

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
    .option(
      "--max-count <n>",
      "maximum live instances (warm + claimed) this pool may ever have at once -- a claim past this queues (up to 30s) instead of " +
        "cold-creating unbounded; omit for no ceiling",
      parsePositiveInt("--max-count"),
    )
    .action(async function (this: Command, id: string, options: { image?: string; vcpu?: number; mem?: number; warmCount: number; maxCount?: number }) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const created = await Pool.create(id, {
          baseUrl,
          authToken: token,
          imageId: options.image,
          vcpuCount: options.vcpu,
          memSizeMib: options.mem,
          warmCount: options.warmCount,
          maxCount: options.maxCount,
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
}

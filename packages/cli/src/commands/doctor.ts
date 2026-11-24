import type { Command } from "commander";
import { getStatus, Sandbox } from "sandkiln";
import { exitCodeFor, formatOutcomes, runChecks, type Check } from "../doctor.js";
import { clientOptions } from "./shared.js";

export function registerDoctorCommand(program: Command): void {
  program
    .command("doctor")
    .description(
      "Diagnose a daemon end to end: reachable, host warnings, then a real create, exec and stop. " +
        "Exits 0 when healthy, 1 on a failure, 2 on warnings only.",
    )
    .action(async function (this: Command) {
      const { baseUrl, token } = clientOptions(this);
      const client = { baseUrl, authToken: token };
      let sandbox: Sandbox | undefined;

      const checks: Check[] = [
        {
          name: "daemon reachable",
          blocking: true,
          run: async () => {
            const status = await getStatus(client);
            return { level: "ok", detail: `sandkilnd ${status.version}, up ${status.uptimeSecs}s` };
          },
        },
        {
          name: "host health",
          run: async () => {
            const status = await getStatus(client);
            return status.warnings.length === 0
              ? { level: "ok", detail: `${status.tapPool.free}/${status.tapPool.size} tap devices free` }
              : { level: "warn", detail: status.warnings.join("; ") };
          },
        },
        {
          name: "create sandbox",
          blocking: true,
          run: async () => {
            sandbox = await Sandbox.create({ ...client, tags: { source: "kiln-doctor" } });
            return { level: "ok", detail: `sandbox ${sandbox.id}` };
          },
        },
        {
          name: "run a command",
          run: async () => {
            const result = await sandbox!.runCommand("echo", ["doctor"]);
            return result.exitCode === 0 && result.stdout.trim() === "doctor"
              ? { level: "ok", detail: "echo round trip" }
              : { level: "fail", detail: `exit ${result.exitCode}, stdout ${JSON.stringify(result.stdout)}` };
          },
        },
        {
          name: "stop sandbox",
          run: async () => {
            await sandbox!.stop({ keep: false });
            sandbox = undefined;
            return { level: "ok", detail: "destroyed, nothing left behind" };
          },
        },
      ];

      const outcomes = await runChecks(checks);
      // A failure after create must not leak the probe sandbox.
      if (sandbox) {
        await sandbox.stop({ keep: false }).catch(() => undefined);
      }
      process.stdout.write(formatOutcomes(outcomes));
      process.exitCode = exitCodeFor(outcomes);
    });
}

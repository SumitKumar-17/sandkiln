import type { Command } from "commander";
import { getStatus } from "sandkiln";
import { formatStatus } from "../format.js";
import { clientOptions, handleApiError } from "./shared.js";

export function registerStatusCommand(program: Command): void {
  program
    .command("status")
    .description("Show daemon health and capacity: counts, tap devices, disk headroom, and warnings. Exits 2 when there are warnings.")
    .option("--json", "print the raw status as JSON")
    .action(async function (this: Command, options: { json?: boolean }) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const status = await getStatus({ baseUrl, authToken: token });
        process.stdout.write(options.json ? JSON.stringify(status, null, 2) + "\n" : formatStatus(status));
        if (status.warnings.length > 0) {
          process.exitCode = 2;
        }
      } catch (error) {
        await handleApiError(error);
      }
    });
}

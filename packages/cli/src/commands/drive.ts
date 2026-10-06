import type { Command } from "commander";
import { Drive } from "sandkiln";
import { formatDriveList, parsePositiveInt } from "../format.js";
import { clientOptions, handleApiError } from "./shared.js";

export function registerDriveCommands(program: Command): void {
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
}

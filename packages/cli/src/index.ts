import { Command } from "commander";
import { registerDriveCommands } from "./commands/drive.js";
import { registerDoctorCommand } from "./commands/doctor.js";
import { registerImageCommands } from "./commands/image.js";
import { registerPoolCommands } from "./commands/pool.js";
import { registerSandboxCommands } from "./commands/sandbox/index.js";
import { registerStatusCommand } from "./commands/status.js";
import { fail } from "./commands/shared.js";

declare const __CLI_VERSION__: string;

const program = new Command();
program
  .name("kiln")
  .version(__CLI_VERSION__)
  .description("Manage sandkiln sandboxes from the command line.")
  .option("--base-url <url>", "daemon URL (default: SANDKILN_DAEMON_URL or http://127.0.0.1:7777)")
  .option("--token <token>", "auth token (default: SANDKILN_AUTH_TOKEN)");

registerSandboxCommands(program);
registerImageCommands(program);
registerDriveCommands(program);
registerPoolCommands(program);
registerStatusCommand(program);
registerDoctorCommand(program);

// Every subcommand's own action handler already catches its errors; this
// is a backstop for anything that escapes one anyway (a bug in a future
// subcommand, or a rejection from commander's own dispatch) so a caller
// always gets a clean stderr message and exit code 1, never a raw stack
// trace.
program.parseAsync(process.argv).catch(async (error: unknown) => {
  await fail(`error: ${error instanceof Error ? error.message : String(error)}`);
});

import type { Command } from "commander";
import { Image } from "sandkiln";
import { formatImageList } from "../format.js";
import { clientOptions, handleApiError } from "./shared.js";

export function registerImageCommands(program: Command): void {
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
}

// Reaching into and out of a sandbox: preview URLs, reverse tunnels and
// remote-storage mounts.
import { Command } from "commander";
import { formatMountList, parsePositiveInt } from "../../format.js";
import { clientOptions, handleApiError } from "../shared.js";
import { attachSandbox } from "./helpers.js";

export function registerTunnel(sandbox: Command): void {
  sandbox
    .command("tunnel <id> <guest-port>")
    .description(
      "Expose a service running on this machine to code inside a sandbox -- the reverse of preview. Runs in the foreground until Ctrl+C.",
    )
    .requiredOption("--local-port <port>", "port on this machine to forward connections to", parsePositiveInt("--local-port"))
    .option("--local-host <host>", "host on this machine to forward to (default: 127.0.0.1)")
    .action(async function (this: Command, id: string, guestPortArg: string, opts: { localPort: number; localHost?: string }) {
      const { baseUrl, token } = clientOptions(this);
      const guestPort = parsePositiveInt("<guest-port>")(guestPortArg);
      try {
        const handle = await attachSandbox(id, baseUrl, token).tunnel(guestPort, { localPort: opts.localPort, localHost: opts.localHost });
        process.stderr.write(`tunnel ${handle.tunnelId} open: sandbox:${guestPort} -> ${opts.localHost ?? "127.0.0.1"}:${opts.localPort}\n`);
        process.stderr.write("Press Ctrl+C to close.\n");
        const close = async () => {
          await handle.close();
          process.exit(0);
        };
        process.on("SIGINT", close);
        process.on("SIGTERM", close);
        await new Promise(() => {});
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerPreview(sandbox: Command): void {
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
}

export function registerMount(sandbox: Command): void {
  sandbox
    .command("mount <id> <bucket> <endpoint> <mount-path>")
    .description(
      "Mount an S3-compatible bucket into a sandbox via 'rclone mount'. Needs the optional FUSE-capable rootfs setup described in SELF_HOSTING.md.",
    )
    .requiredOption("--access-key <key>", "S3-compatible access key")
    .requiredOption("--secret-key <key>", "S3-compatible secret key")
    .option("--read-only", "mount read-only")
    .action(async function (
      this: Command,
      id: string,
      bucket: string,
      endpoint: string,
      mountPath: string,
      opts: { accessKey: string; secretKey: string; readOnly?: boolean },
    ) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const mount = await attachSandbox(id, baseUrl, token).mount({
          bucket,
          endpoint,
          accessKey: opts.accessKey,
          secretKey: opts.secretKey,
          mountPath,
          readOnly: opts.readOnly,
        });
        process.stdout.write(`${mount.id}\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerMounts(sandbox: Command): void {
  sandbox
    .command("mounts <id>")
    .description("List remote-storage mounts currently active in a sandbox.")
    .action(async function (this: Command, id: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const mounts = await attachSandbox(id, baseUrl, token).listMounts();
        process.stdout.write(formatMountList(mounts));
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerUnmount(sandbox: Command): void {
  sandbox
    .command("unmount <id> <mount-id>")
    .description("Unmount and tear down one remote-storage mount (see 'kiln sandbox mounts' for the mount id).")
    .action(async function (this: Command, id: string, mountId: string) {
      const { baseUrl, token } = clientOptions(this);
      try {
        await attachSandbox(id, baseUrl, token).unmount(mountId);
        process.stdout.write(`${mountId} unmounted\n`);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

/** Entry point: stdio transport plus guaranteed cleanup.
 *
 * stdout carries the protocol, so every diagnostic goes to stderr. */

import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { ConfigError, parseConfig } from "./config.js";
import { createServer } from "./server.js";

const CLEANUP_DEADLINE_MS = 15_000;

async function main(): Promise<void> {
  const config = parseConfig(process.env);
  const { server, session } = createServer(config);

  let shuttingDown = false;
  const shutdown = async (reason: string, code: number): Promise<void> => {
    if (shuttingDown) return;
    shuttingDown = true;
    const forced = setTimeout(() => process.exit(1), CLEANUP_DEADLINE_MS);
    forced.unref();
    const ids = session.ids();
    if (ids.length > 0) {
      const failed = await session.cleanup(config.keepOnExit);
      const verb = config.keepOnExit ? "snapshotted" : "destroyed";
      console.error(`sandkiln-mcp: ${reason}; ${ids.length - failed.length}/${ids.length} sandboxes ${verb}`);
      if (failed.length > 0) console.error(`sandkiln-mcp: could not stop: ${failed.join(", ")}`);
    }
    await server.close().catch(() => undefined);
    process.exit(code);
  };

  process.on("SIGTERM", () => void shutdown("SIGTERM", 0));
  process.on("SIGINT", () => void shutdown("SIGINT", 0));
  process.stdin.on("end", () => void shutdown("stdin closed", 0));
  process.stdin.on("close", () => void shutdown("stdin closed", 0));

  await server.connect(new StdioServerTransport());
  console.error(`sandkiln-mcp ready (daemon ${config.baseUrl ?? "default"}, max ${config.maxSandboxes} sandboxes)`);
}

main().catch((err: unknown) => {
  console.error(`sandkiln-mcp: ${err instanceof ConfigError ? err.message : err instanceof Error ? err.stack : String(err)}`);
  process.exit(1);
});

/** Daemon health tool. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { getStatus } from "sandkiln";
import { defineTool, type ToolContext } from "./register.js";

export function registerStatusTools(server: McpServer, ctx: ToolContext): void {
  defineTool(
    server,
    ctx,
    "daemon_status",
    {
      title: "Daemon status",
      description:
        "Report sandkiln daemon health and capacity: version, uptime, live sandboxes, held snapshots, free network devices, disk headroom, and warnings to act on. " +
        "Call it first when a create fails to see whether the host is out of capacity.",
      inputSchema: {},
      annotations: { title: "Daemon status", readOnlyHint: true, openWorldHint: false },
    },
    async () => ({ ...(await getStatus(ctx.client)) }),
  );
}

/** Read-only resources: a client can attach these to context without a tool call. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { Sandbox, getStatus } from "sandkiln";
import type { ToolContext } from "./tools/register.js";

function json(uri: string, data: unknown) {
  return { contents: [{ uri, mimeType: "application/json", text: JSON.stringify(data, null, 2) }] };
}

export function registerResources(server: McpServer, ctx: ToolContext): void {
  server.registerResource(
    "sandboxes",
    "sandkiln://sandboxes",
    { title: "Live sandboxes", description: "Every live sandbox on the daemon, with tags and which ones this session created.", mimeType: "application/json" },
    async (uri) => {
      const list = await Sandbox.list(ctx.client);
      return json(
        uri.href,
        list.map((s) => ({
          id: s.id,
          name: s.name ?? null,
          tags: s.tags,
          created_at: s.createdAt.toISOString(),
          created_by_this_session: ctx.session.owns(s.id),
        })),
      );
    },
  );

  server.registerResource(
    "status",
    "sandkiln://status",
    { title: "Daemon status", description: "Daemon health, capacity and warnings.", mimeType: "application/json" },
    async (uri) => json(uri.href, await getStatus(ctx.client)),
  );
}

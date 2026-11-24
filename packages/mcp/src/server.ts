/** Wires configuration, the session, tools and resources into one MCP server. */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { Sandbox, SandkilnApiError } from "sandkiln";
import type { McpConfig } from "./config.js";
import { registerResources } from "./resources.js";
import { SandboxSession } from "./session.js";
import { registerExecTools } from "./tools/exec.js";
import { registerFileTools } from "./tools/files.js";
import type { ToolContext } from "./tools/register.js";
import { registerSandboxTools } from "./tools/sandboxes.js";
import { registerSnapshotTools } from "./tools/snapshots.js";
import { registerStatusTools } from "./tools/status.js";

export const SERVER_NAME = "sandkiln-mcp";
export const SERVER_VERSION = "0.1.0";

const INSTRUCTIONS =
  "Tools for running untrusted or generated code in hardware-isolated sandkiln microVMs. " +
  "Typical flow: sandbox_create, then sandbox_exec / sandbox_write_file / sandbox_read_file, then sandbox_stop. " +
  "Stop sandboxes you no longer need; any still running when the session ends are stopped automatically.";

export interface SandkilnMcp {
  server: McpServer;
  session: SandboxSession;
}

export function createServer(config: McpConfig): SandkilnMcp {
  const client = {
    ...(config.baseUrl !== undefined ? { baseUrl: config.baseUrl } : {}),
    ...(config.authToken !== undefined ? { authToken: config.authToken } : {}),
  };

  const session = new SandboxSession(config.maxSandboxes, async (id, keep) => {
    try {
      await Sandbox.attach(id, client).stop({ keep });
    } catch (err) {
      // Already gone (stopped by the model or reaped by the daemon) counts as cleaned up.
      if (!(err instanceof SandkilnApiError && err.status === 404)) throw err;
    }
  });

  const server = new McpServer({ name: SERVER_NAME, version: SERVER_VERSION }, { instructions: INSTRUCTIONS });
  const ctx: ToolContext = { config, session, client };
  registerSandboxTools(server, ctx);
  registerExecTools(server, ctx);
  registerFileTools(server, ctx);
  registerSnapshotTools(server, ctx);
  registerStatusTools(server, ctx);
  registerResources(server, ctx);
  return { server, session };
}

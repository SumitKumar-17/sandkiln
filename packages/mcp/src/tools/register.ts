/** The one place a tool handler becomes an MCP tool: validation comes from
 * the zod shape, and every thrown error is converted to a structured,
 * `isError` result so a failure never surfaces as a protocol error. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import type { CallToolResult, ToolAnnotations } from "@modelcontextprotocol/sdk/types.js";
import { Sandbox, type SandboxOptions } from "sandkiln";
import type { z } from "zod";
import type { McpConfig } from "../config.js";
import { toToolError } from "../errors.js";
import type { SandboxSession } from "../session.js";

export interface ToolContext {
  config: McpConfig;
  session: SandboxSession;
  /** Connection options passed to every SDK call. */
  client: SandboxOptions;
}

export function attach(ctx: ToolContext, id: string): Sandbox {
  return Sandbox.attach(id, ctx.client);
}

export interface ToolSpec<S extends z.ZodRawShape> {
  title: string;
  description: string;
  inputSchema: S;
  annotations: ToolAnnotations;
}

export function success(data: Record<string, unknown>): CallToolResult {
  return {
    content: [{ type: "text", text: JSON.stringify(data, null, 2) }],
    structuredContent: data,
  };
}

export function failure(err: unknown, ctx: ToolContext): CallToolResult {
  const error = toToolError(err, ctx.config.baseUrl);
  return { isError: true, content: [{ type: "text", text: JSON.stringify({ error }, null, 2) }] };
}

export function defineTool<S extends z.ZodRawShape>(
  server: McpServer,
  ctx: ToolContext,
  name: string,
  spec: ToolSpec<S>,
  handler: (args: z.infer<z.ZodObject<S>>) => Promise<Record<string, unknown>>,
): void {
  server.registerTool(name, spec, (async (args: z.infer<z.ZodObject<S>>) => {
    try {
      return success(await handler(args));
    } catch (err) {
      return failure(err, ctx);
    }
  }) as never);
}

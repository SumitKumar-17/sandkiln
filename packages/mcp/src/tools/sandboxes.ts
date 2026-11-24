/** Sandbox lifecycle tools: create, list, stop. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { Sandbox } from "sandkiln";
import { z } from "zod";
import { attach, defineTool, type ToolContext } from "./register.js";

const sandboxId = z.string().min(1).describe("Sandbox id returned by sandbox_create.");

export function registerSandboxTools(server: McpServer, ctx: ToolContext): void {
  defineTool(
    server,
    ctx,
    "sandbox_create",
    {
      title: "Create sandbox",
      description:
        "Boot a new hardware-isolated microVM sandbox and return its id. Use it to run untrusted or generated code safely. " +
        "The sandbox is stopped automatically when this MCP session ends unless the server was configured to keep it. " +
        "Egress is unrestricted by default; use egress=deny_all for code that must not reach the network.",
      inputSchema: {
        image: z.string().min(1).optional().describe("Registered image id to boot from. Omit for the default image."),
        vcpu: z.number().int().min(1).optional().describe("vCPU count. Omit for the daemon default; the daemon rejects values past its ceiling."),
        mem_mib: z.number().int().min(1).optional().describe("Memory in MiB. Omit for the daemon default."),
        name: z.string().min(1).optional().describe("Optional unique name for the sandbox."),
        tags: z.record(z.string(), z.string()).optional().describe("Key/value labels, usable to filter sandbox_list."),
        env: z.record(z.string(), z.string()).optional().describe("Environment variables set for every command in this sandbox."),
        egress: z
          .enum(["allow_all", "deny_all"])
          .optional()
          .describe("Outbound network mode. deny_all blocks all outbound traffic except what allow_cidrs permits."),
        allow_cidrs: z.array(z.string()).optional().describe("With egress=deny_all: CIDR ranges still reachable, for example 10.0.0.0/8."),
      },
      annotations: { title: "Create sandbox", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
    },
    async (a) => {
      ctx.session.assertCanCreate();
      const sandbox = await Sandbox.create({
        ...ctx.client,
        ...(a.image !== undefined ? { imageId: a.image } : {}),
        ...(a.vcpu !== undefined ? { vcpuCount: a.vcpu } : {}),
        ...(a.mem_mib !== undefined ? { memSizeMib: a.mem_mib } : {}),
        ...(a.name !== undefined ? { name: a.name } : {}),
        ...(a.tags !== undefined ? { tags: a.tags } : {}),
        ...(a.env !== undefined ? { env: a.env } : {}),
        ...(a.egress !== undefined ? { egress: { mode: a.egress, ...(a.allow_cidrs ? { allowCidrs: a.allow_cidrs } : {}) } } : {}),
      });
      ctx.session.track(sandbox.id);
      return { sandbox_id: sandbox.id, stopped_on_exit: !ctx.config.keepOnExit };
    },
  );

  defineTool(
    server,
    ctx,
    "sandbox_list",
    {
      title: "List sandboxes",
      description: "List live sandboxes on the daemon, optionally filtered by tags. Marks the ones created by this session.",
      inputSchema: { tags: z.record(z.string(), z.string()).optional().describe("Only sandboxes carrying every given tag.") },
      annotations: { title: "List sandboxes", readOnlyHint: true, openWorldHint: false },
    },
    async (a) => {
      const list = await Sandbox.list({ ...ctx.client, ...(a.tags ? { tags: a.tags } : {}) });
      return {
        count: list.length,
        sandboxes: list.map((s) => ({
          id: s.id,
          name: s.name ?? null,
          tags: s.tags,
          created_at: s.createdAt.toISOString(),
          created_by_this_session: ctx.session.owns(s.id),
        })),
      };
    },
  );

  defineTool(
    server,
    ctx,
    "sandbox_stop",
    {
      title: "Stop sandbox",
      description:
        "Stop a sandbox. By default (keep=false) it is destroyed and its disk is discarded, which avoids leaking resources. " +
        "Pass keep=true to snapshot it first so it can be brought back with sandbox_resume.",
      inputSchema: {
        sandbox_id: sandboxId,
        keep: z.boolean().default(false).describe("true: preserve state as a snapshot and return its id. false: destroy."),
      },
      annotations: { title: "Stop sandbox", readOnlyHint: false, destructiveHint: true, idempotentHint: false, openWorldHint: false },
    },
    async (a) => {
      const result = await attach(ctx, a.sandbox_id).stop({ keep: a.keep });
      ctx.session.forget(a.sandbox_id);
      return { sandbox_id: a.sandbox_id, kept: result.kept, snapshot_id: result.snapshotId };
    },
  );
}

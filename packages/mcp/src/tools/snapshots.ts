/** Snapshot tools: snapshot, list, resume, fork. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { Sandbox } from "sandkiln";
import { z } from "zod";
import { attach, defineTool, type ToolContext } from "./register.js";

const snapshotId = z.string().min(1).describe("Snapshot id from sandbox_snapshot or snapshot_list.");

export function registerSnapshotTools(server: McpServer, ctx: ToolContext): void {
  defineTool(
    server,
    ctx,
    "sandbox_snapshot",
    {
      title: "Snapshot sandbox",
      description:
        "Freeze a sandbox's memory and disk into a snapshot and return its id. The sandbox itself is retired by this call " +
        "(its id stops working); bring the state back with sandbox_resume or sandbox_fork. Snapshots persist on the daemon after this session ends.",
      inputSchema: { sandbox_id: z.string().min(1).describe("Sandbox id from sandbox_create.") },
      annotations: { title: "Snapshot sandbox", readOnlyHint: false, destructiveHint: true, idempotentHint: false, openWorldHint: false },
    },
    async (a) => {
      const id = await attach(ctx, a.sandbox_id).snapshot();
      ctx.session.forget(a.sandbox_id);
      return { snapshot_id: id, retired_sandbox_id: a.sandbox_id };
    },
  );

  defineTool(
    server,
    ctx,
    "snapshot_list",
    {
      title: "List snapshots",
      description: "List held snapshots, with the live sandbox currently forked from each, if any.",
      inputSchema: { source_sandbox_id: z.string().min(1).optional().describe("Only the snapshot taken from this sandbox id.") },
      annotations: { title: "List snapshots", readOnlyHint: true, openWorldHint: false },
    },
    async (a) => {
      const list = await Sandbox.listSnapshots({ ...ctx.client, ...(a.source_sandbox_id ? { sourceSandboxId: a.source_sandbox_id } : {}) });
      return {
        count: list.length,
        snapshots: list.map((s) => ({
          id: s.id,
          source_sandbox_id: s.sourceSandboxId,
          name: s.name ?? null,
          tags: s.tags,
          created_at: s.createdAt.toISOString(),
          forked_into: s.forkedInto,
        })),
      };
    },
  );

  defineTool(
    server,
    ctx,
    "sandbox_resume",
    {
      title: "Resume sandbox from snapshot",
      description:
        "Boot a sandbox from a snapshot, consuming the snapshot: it is gone afterwards. The resumed sandbox gets a new id " +
        "and is stopped when this session ends. Use sandbox_fork instead to keep the snapshot.",
      inputSchema: { snapshot_id: snapshotId },
      annotations: { title: "Resume sandbox from snapshot", readOnlyHint: false, destructiveHint: true, idempotentHint: false, openWorldHint: false },
    },
    async (a) => {
      ctx.session.assertCanCreate();
      const sandbox = await Sandbox.resume(a.snapshot_id, ctx.client);
      ctx.session.track(sandbox.id);
      return { sandbox_id: sandbox.id, snapshot_consumed: true };
    },
  );

  defineTool(
    server,
    ctx,
    "sandbox_fork",
    {
      title: "Fork sandbox from snapshot",
      description:
        "Boot a new sandbox from a snapshot without consuming it, so the same starting state can be explored again. " +
        "At most one live fork per snapshot exists at a time: a second fork fails with a conflict until the first is stopped.",
      inputSchema: { snapshot_id: snapshotId },
      annotations: { title: "Fork sandbox from snapshot", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
    },
    async (a) => {
      ctx.session.assertCanCreate();
      const sandbox = await Sandbox.fork(a.snapshot_id, ctx.client);
      ctx.session.track(sandbox.id);
      return { sandbox_id: sandbox.id, snapshot_consumed: false };
    },
  );
}

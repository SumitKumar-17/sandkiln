/** Filesystem tools: write, read, list. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { ToolFailure } from "../errors.js";
import { decodeFile } from "../truncate.js";
import { attach, defineTool, type ToolContext } from "./register.js";

/** Writes above this are refused: they would travel through the model's context twice. */
export const MAX_WRITE_BYTES = 4 * 1024 * 1024;
export const MAX_LIST_ENTRIES = 500;

const sandboxId = z.string().min(1).describe("Sandbox id from sandbox_create.");
const path = z.string().min(1).describe("Absolute path inside the sandbox.");

export function registerFileTools(server: McpServer, ctx: ToolContext): void {
  defineTool(
    server,
    ctx,
    "sandbox_write_file",
    {
      title: "Write file in sandbox",
      description: `Create or overwrite a file inside a sandbox. Content is UTF-8 text by default; use encoding=base64 for binary data. Limit ${MAX_WRITE_BYTES} bytes.`,
      inputSchema: {
        sandbox_id: sandboxId,
        path,
        content: z.string().describe("File content, interpreted per `encoding`."),
        encoding: z.enum(["utf8", "base64"]).default("utf8").describe("How `content` is encoded."),
      },
      annotations: { title: "Write file in sandbox", readOnlyHint: false, destructiveHint: true, idempotentHint: true, openWorldHint: false },
    },
    async (a) => {
      const bytes = a.encoding === "base64" ? Buffer.from(a.content, "base64") : Buffer.from(a.content, "utf8");
      if (bytes.length > MAX_WRITE_BYTES) {
        throw new ToolFailure("invalid_request", `content is ${bytes.length} bytes, over the ${MAX_WRITE_BYTES} byte limit`);
      }
      await attach(ctx, a.sandbox_id).writeFile(a.path, bytes);
      return { path: a.path, bytes_written: bytes.length };
    },
  );

  defineTool(
    server,
    ctx,
    "sandbox_read_file",
    {
      title: "Read file from sandbox",
      description:
        "Read a file from a sandbox. Text comes back as UTF-8; binary files come back base64 with encoding=base64. " +
        "Large files are cut to max_bytes and flagged as truncated.",
      inputSchema: {
        sandbox_id: sandboxId,
        path,
        max_bytes: z.number().int().min(1).max(1024 * 1024).optional().describe("Most bytes of the file to return. Defaults to the server output cap."),
      },
      annotations: { title: "Read file from sandbox", readOnlyHint: true, openWorldHint: false },
    },
    async (a) => {
      const bytes = await attach(ctx, a.sandbox_id).readFile(a.path);
      const limit = Math.min(a.max_bytes ?? ctx.config.maxOutputBytes, 1024 * 1024);
      const file = decodeFile(bytes, limit);
      return {
        path: a.path,
        encoding: file.encoding,
        content: file.content,
        total_bytes: file.totalBytes,
        returned_bytes: file.returnedBytes,
        truncated: file.truncated,
      };
    },
  );

  defineTool(
    server,
    ctx,
    "sandbox_list_dir",
    {
      title: "List directory in sandbox",
      description: `List the entries of a directory inside a sandbox: name, type, size, mode and modification time. At most ${MAX_LIST_ENTRIES} entries are returned.`,
      inputSchema: { sandbox_id: sandboxId, path },
      annotations: { title: "List directory in sandbox", readOnlyHint: true, openWorldHint: false },
    },
    async (a) => {
      const entries = await attach(ctx, a.sandbox_id).listDir(a.path);
      return {
        path: a.path,
        total_entries: entries.length,
        truncated: entries.length > MAX_LIST_ENTRIES,
        entries: entries.slice(0, MAX_LIST_ENTRIES).map((e) => ({
          name: e.name,
          type: e.isSymlink ? "symlink" : e.isDir ? "dir" : "file",
          size: e.size,
          mode: e.mode.toString(8),
          modified_at: e.mtime.toISOString(),
        })),
      };
    },
  );
}

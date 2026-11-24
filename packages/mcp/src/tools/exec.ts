/** Command execution tool. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { SandkilnExecTimeoutError } from "sandkiln";
import { z } from "zod";
import { execTimeoutFailure } from "../exec-timeout.js";
import { truncateText } from "../truncate.js";
import { attach, defineTool, type ToolContext } from "./register.js";

export const DEFAULT_EXEC_TIMEOUT_S = 60;
export const MAX_EXEC_TIMEOUT_S = 600;

export function registerExecTools(server: McpServer, ctx: ToolContext): void {
  defineTool(
    server,
    ctx,
    "sandbox_exec",
    {
      title: "Run command in sandbox",
      description:
        "Run one command inside a sandbox and wait for it to finish. The command is executed directly, not through a shell: " +
        "to use pipes, redirects or globbing, call command=\"sh\" with args=[\"-c\", \"...\"]. " +
        "Returns stdout, stderr and the exit code; a nonzero exit code is a normal result, not a tool error. " +
        "Output beyond the size cap keeps its head and tail with a note in between.",
      inputSchema: {
        sandbox_id: z.string().min(1).describe("Sandbox id from sandbox_create."),
        command: z.string().min(1).describe("Executable to run, for example python3 or sh."),
        args: z.array(z.string()).default([]).describe("Arguments, one array element each."),
        env: z.record(z.string(), z.string()).optional().describe("Extra environment variables for this command only."),
        timeout_seconds: z
          .number()
          .int()
          .min(1)
          .max(MAX_EXEC_TIMEOUT_S)
          .default(DEFAULT_EXEC_TIMEOUT_S)
          .describe(
            "Wall-clock limit enforced inside the sandbox. On expiry the command and its child processes are killed and the call returns a timeout error carrying the output captured so far.",
          ),
      },
      annotations: { title: "Run command in sandbox", readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: true },
    },
    async (a) => {
      const sandbox = attach(ctx, a.sandbox_id);
      let result;
      try {
        result = await sandbox.runCommand(a.command, a.args, {
          ...(a.env ? { env: a.env } : {}),
          timeoutSeconds: a.timeout_seconds,
        });
      } catch (err) {
        if (err instanceof SandkilnExecTimeoutError) throw execTimeoutFailure(err, ctx.config.maxOutputBytes);
        throw err;
      }
      // The budget is split so that stdout and stderr together stay within the cap.
      const budget = Math.floor(ctx.config.maxOutputBytes / 2);
      const stdout = truncateText(result.stdout, budget);
      const stderr = truncateText(result.stderr, budget);
      return {
        exit_code: result.exitCode,
        stdout: stdout.text,
        stderr: stderr.text,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        stdout_total_bytes: stdout.totalBytes,
        stderr_total_bytes: stderr.totalBytes,
      };
    },
  );
}

// Running code in a sandbox: one-shot exec, the interactive pty, and
// background exec-stream sessions with their logs.
import { Command } from "commander";
import { Sandbox, SandkilnExecTimeoutError } from "sandkiln";
import { execTimeoutReport, formatExecStreamList, parsePositiveInt } from "../../format.js";
import { clientOptions, handleApiError } from "../shared.js";
import { attachSandbox, envOption } from "./helpers.js";

/** Shared by both `exec-stream` (attach right after starting) and `logs`
 * (attach to an already-running or already-finished session): prints a
 * replay-then-live-tail of the session's output to stdout, and resolves
 * with the remote command's own exit code once it's known (from the
 * daemon's "[process exited with code N]" notice, printed to stderr
 * alongside every other bracketed notice like a truncation warning) --
 * `undefined` if the connection just closes without ever seeing one
 * (e.g. the daemon restarted mid-session). */
async function followLogs(sbx: Sandbox, sessionId: string): Promise<number | undefined> {
  const ws = sbx.attachLogs(sessionId);
  ws.binaryType = "arraybuffer";

  return new Promise<number | undefined>((resolve) => {
    let exitCode: number | undefined;
    ws.addEventListener("message", (event) => {
      if (typeof event.data === "string") {
        const match = /^\[process exited with code (-?\d+)\]/.exec(event.data);
        if (match) {
          exitCode = Number(match[1]);
        }
        process.stderr.write(event.data);
        return;
      }
      const buf = event.data instanceof ArrayBuffer ? Buffer.from(event.data) : Buffer.from(String(event.data));
      process.stdout.write(buf);
    });
    const end = () => resolve(exitCode);
    ws.addEventListener("close", end);
    ws.addEventListener("error", end);
  });
}

export function registerExec(sandbox: Command): void {
  sandbox
    .command("exec <id> <command> [args...]")
    .description(
      "Run a command inside a sandbox. Exits with the command's own exit code, or 124 (like coreutils timeout) " +
        "if --timeout expired and the command was killed.",
    )
    .addOption(envOption("environment variable for this one call (repeatable); overrides a same-named one baked in at create time"))
    .option(
      "--timeout <seconds>",
      "kill the command (its whole process group) after this many seconds; the daemon's exec_timeout_secs is both the default and the maximum",
      parsePositiveInt("--timeout"),
    )
    .action(async function (
      this: Command,
      id: string,
      command: string,
      args: string[],
      opts: { env: Record<string, string>; timeout?: number },
    ) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const result = await attachSandbox(id, baseUrl, token).runCommand(command, args, {
          env: Object.keys(opts.env).length > 0 ? opts.env : undefined,
          timeoutSeconds: opts.timeout,
        });
        process.stdout.write(result.stdout);
        process.stderr.write(result.stderr);
        process.exit(result.exitCode);
      } catch (error) {
        if (error instanceof SandkilnExecTimeoutError) {
          const report = execTimeoutReport(error);
          process.stdout.write(report.stdout);
          process.stderr.write(report.stderr);
          process.exit(report.exitCode);
        }
        await handleApiError(error);
      }
    });
}

export function registerPty(sandbox: Command): void {
  sandbox
    .command("pty <id>")
    .description("Open a live, interactive shell session inside a sandbox. Exit the remote shell (or Ctrl+D) to end the session.")
    .action(async function (this: Command, id: string) {
      const { baseUrl, token } = clientOptions(this);
      const cols = process.stdout.columns || 80;
      const rows = process.stdout.rows || 24;

      let ws: WebSocket;
      try {
        ws = attachSandbox(id, baseUrl, token).pty({ cols, rows });
      } catch (error) {
        await handleApiError(error);
        return;
      }
      ws.binaryType = "arraybuffer";

      const wasRaw = process.stdin.isTTY ? process.stdin.isRaw : undefined;
      const restoreTerminal = () => {
        if (process.stdin.isTTY && wasRaw !== undefined) {
          process.stdin.setRawMode(wasRaw);
        }
        process.stdin.pause();
        process.stdin.removeAllListeners("data");
      };

      await new Promise<void>((resolve) => {
        ws.addEventListener("open", () => {
          if (process.stdin.isTTY) {
            process.stdin.setRawMode(true);
          }
          process.stdin.resume();
          // Every keystroke goes straight to the remote shell, including
          // Ctrl+C -- raw mode means Node never intercepts it as SIGINT,
          // the same way a real terminal/SSH client leaves Ctrl+C to
          // whatever's running remotely rather than killing the local
          // process.
          process.stdin.on("data", (chunk: Buffer) => ws.send(chunk));
        });

        ws.addEventListener("message", (event) => {
          const buf = event.data instanceof ArrayBuffer ? Buffer.from(event.data) : Buffer.from(String(event.data));
          process.stdout.write(buf);
        });

        const end = () => {
          restoreTerminal();
          resolve();
        };
        ws.addEventListener("close", end);
        ws.addEventListener("error", end);
      });
    });
}

export function registerExecStream(sandbox: Command): void {
  sandbox
    .command("exec-stream <id> <command> [args...]")
    .description(
      "Run a long-running command inside a sandbox in the background and follow its output live (replay so far, then a live tail) until it exits. Ctrl+C detaches without stopping the command -- reattach later with 'kiln sandbox logs <id> <session-id>'.",
    )
    .addOption(envOption("environment variable for this one call (repeatable); overrides a same-named one baked in at create time"))
    .action(async function (this: Command, id: string, command: string, args: string[], opts: { env: Record<string, string> }) {
      const { baseUrl, token } = clientOptions(this);
      try {
        const sbx = attachSandbox(id, baseUrl, token);
        const sessionId = await sbx.execStream(command, args, { env: Object.keys(opts.env).length > 0 ? opts.env : undefined });
        process.stderr.write(`session ${sessionId} started\n`);
        const exitCode = await followLogs(sbx, sessionId);
        process.exit(exitCode ?? 0);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

export function registerLogs(sandbox: Command): void {
  sandbox
    .command("logs <id> [session-id]")
    .description(
      "Without <session-id>: list this sandbox's exec-stream sessions. With one: attach and follow it (replay so far, then a live tail), the same way as right after 'exec-stream' starts it -- works any number of times, including after the process has already finished.",
    )
    .action(async function (this: Command, id: string, sessionId: string | undefined) {
      const { baseUrl, token } = clientOptions(this);
      const sbx = attachSandbox(id, baseUrl, token);
      try {
        if (sessionId === undefined) {
          const sessions = await sbx.listExecStreams();
          process.stdout.write(formatExecStreamList(sessions));
          return;
        }
        const exitCode = await followLogs(sbx, sessionId);
        process.exit(exitCode ?? 0);
      } catch (error) {
        await handleApiError(error);
      }
    });
}

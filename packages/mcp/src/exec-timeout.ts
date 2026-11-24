import { SandkilnExecTimeoutError } from "sandkiln";
import { ToolFailure } from "./errors.js";
import { truncateText } from "./truncate.js";

/** The tool error for a command the sandbox killed at `timeout_seconds`.
 * The partial output rides along (bounded like a normal result) because what
 * a command printed before it hung is usually the clue to why. */
export function execTimeoutFailure(err: SandkilnExecTimeoutError, outputBudgetBytes: number): ToolFailure {
  const budget = Math.floor(outputBudgetBytes / 2);
  const stdout = truncateText(err.stdout, budget);
  const stderr = truncateText(err.stderr, budget);
  return new ToolFailure(
    "timeout",
    `command did not finish within ${err.timeoutSeconds}s and was killed`,
    "The command and its child processes were killed inside the sandbox, so nothing is left running. " +
      "Raise timeout_seconds, or run it in the background (for example with nohup and output to a file) and poll.",
    {
      stdout: stdout.text,
      stderr: stderr.text,
      stdout_truncated: stdout.truncated,
      stderr_truncated: stderr.truncated,
    },
  );
}

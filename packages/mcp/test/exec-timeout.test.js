import { test } from "node:test";
import assert from "node:assert/strict";
import { SandkilnExecTimeoutError } from "sandkiln";
import { toToolError } from "../dist/errors.js";
import { execTimeoutFailure } from "../dist/exec-timeout.js";

const killed = (stdout, stderr) => new SandkilnExecTimeoutError("timed out", { stdout, stderr, timeoutSeconds: 2 });

test("a killed command becomes a timeout tool error that says nothing is still running", () => {
  const e = toToolError(execTimeoutFailure(killed("partial\n", "warn\n"), 1000), undefined);
  assert.equal(e.kind, "timeout");
  assert.match(e.message, /within 2s and was killed/);
  assert.match(e.hint, /nothing is left running/);
  assert.equal(e.details.stdout, "partial\n");
  assert.equal(e.details.stderr, "warn\n");
  assert.equal(e.details.stdout_truncated, false);
});

test("partial output is bounded like a normal result", () => {
  const e = toToolError(execTimeoutFailure(killed("x".repeat(5000), ""), 1000), undefined);
  assert.equal(e.details.stdout_truncated, true);
  assert.ok(e.details.stdout.length < 1000);
});

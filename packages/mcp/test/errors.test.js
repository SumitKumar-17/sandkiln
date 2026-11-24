import { test } from "node:test";
import assert from "node:assert/strict";
import { SandkilnApiError } from "sandkiln";
import { ToolFailure, toToolError } from "../dist/errors.js";

test("maps daemon status codes to stable kinds", () => {
  const cases = [
    [400, "invalid_request"],
    [401, "unauthorized"],
    [404, "not_found"],
    [409, "conflict"],
    [429, "rate_limited"],
    [500, "daemon_error"],
  ];
  for (const [status, kind] of cases) {
    const e = toToolError(new SandkilnApiError(status, "boom"), undefined);
    assert.equal(e.kind, kind);
    assert.equal(e.status, status);
    assert.equal(e.message, "boom");
  }
  assert.ok(toToolError(new SandkilnApiError(404, "x"), undefined).hint);
});

test("a refused connection becomes daemon_unreachable naming the url", () => {
  const err = new TypeError("fetch failed", { cause: new Error("connect ECONNREFUSED 127.0.0.1:1") });
  const e = toToolError(err, "http://127.0.0.1:1");
  assert.equal(e.kind, "daemon_unreachable");
  assert.match(e.message, /127\.0\.0\.1:1/);
});

test("ToolFailure keeps its kind and hint; unknown errors are internal", () => {
  const e = toToolError(new ToolFailure("timeout", "slow", "retry"), undefined);
  assert.deepEqual(e, { kind: "timeout", message: "slow", hint: "retry" });
  assert.equal(toToolError(new Error("odd"), undefined).kind, "internal");
});

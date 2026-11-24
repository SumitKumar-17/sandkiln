import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { Sandbox, SandkilnApiError, SandkilnExecTimeoutError } from "../dist/index.js";

/** A daemon stand-in that records the last exec body and replies with `reply`. */
async function fakeDaemon(reply) {
  const seen = { body: undefined };
  const server = http.createServer((req, res) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      seen.body = JSON.parse(raw);
      res.writeHead(reply.status, { "content-type": "application/json" });
      res.end(JSON.stringify(reply.body));
    });
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const baseUrl = `http://127.0.0.1:${server.address().port}`;
  return { seen, baseUrl, close: () => server.close() };
}

test("timeoutSeconds is sent as timeout_secs and omitted when unset", async () => {
  const d = await fakeDaemon({ status: 200, body: { stdout: "", stderr: "", exit_code: 0 } });
  try {
    const sb = Sandbox.attach("abc", { baseUrl: d.baseUrl });
    await sb.runCommand("true", [], { timeoutSeconds: 7 });
    assert.equal(d.seen.body.timeout_secs, 7);
    await sb.runCommand("true");
    assert.equal("timeout_secs" in d.seen.body, false);
  } finally { d.close(); }
});

test("a killed command throws SandkilnExecTimeoutError with the partial output", async () => {
  const d = await fakeDaemon({
    status: 504,
    body: { error: "command timed out after 2s and was killed", timed_out: true, timeout_secs: 2, stdout: "partial\n", stderr: "warn\n" },
  });
  try {
    const sb = Sandbox.attach("abc", { baseUrl: d.baseUrl });
    await assert.rejects(sb.runCommand("sleep", ["30"], { timeoutSeconds: 2 }), (err) => {
      assert.ok(err instanceof SandkilnExecTimeoutError);
      assert.ok(err instanceof SandkilnApiError);
      assert.equal(err.status, 504);
      assert.equal(err.stdout, "partial\n");
      assert.equal(err.stderr, "warn\n");
      assert.equal(err.timeoutSeconds, 2);
      return true;
    });
  } finally { d.close(); }
});

test("a bare 504 (no reply from the guest) stays a plain SandkilnApiError", async () => {
  const d = await fakeDaemon({ status: 504, body: { error: "no reply from the sandbox within 12s" } });
  try {
    const sb = Sandbox.attach("abc", { baseUrl: d.baseUrl });
    await assert.rejects(sb.runCommand("sleep", ["30"], { timeoutSeconds: 2 }), (err) => {
      assert.ok(err instanceof SandkilnApiError);
      assert.ok(!(err instanceof SandkilnExecTimeoutError));
      return true;
    });
  } finally { d.close(); }
});

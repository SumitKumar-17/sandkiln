import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const cli = fileURLToPath(new URL("../dist/index.js", import.meta.url));

async function run(reply) {
  const server = http.createServer((req, res) => {
    req.resume();
    req.on("end", () => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(reply));
    });
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const url = `http://127.0.0.1:${server.address().port}`;
  try {
    return await new Promise((resolve) => {
      const child = spawn(process.execPath, [cli, "--base-url", url, "sandbox", "exec", "abc", "nosuch"]);
      let stderr = "";
      child.stderr.on("data", (c) => (stderr += c));
      child.on("close", (code) => resolve({ code, stderr }));
    });
  } finally { server.close(); }
}

test("kiln sandbox exec exits 127 and prints the reason when the command does not exist", async () => {
  const { code, stderr } = await run({ stdout: "", stderr: "nosuch: command not found\n", exit_code: 127 });
  assert.equal(code, 127);
  assert.equal(stderr, "nosuch: command not found\n");
});

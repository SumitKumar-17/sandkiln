import { test } from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { Sandbox } from "../dist/index.js";

// The guest answers a command that cannot start with a normal exec result
// (shell convention: 127 not found, 126 not executable), so runCommand returns
// it and does not throw.
async function fakeDaemon(body) {
  const server = http.createServer((req, res) => {
    req.resume();
    req.on("end", () => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    });
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  return { baseUrl: `http://127.0.0.1:${server.address().port}`, close: () => server.close() };
}

for (const [name, code, stderr] of [
  ["a missing command", 127, "nosuch: command not found\n"],
  ["a directory or non-executable file", 126, "/tmp: Is a directory\n"],
]) {
  test(`${name} resolves with exit code ${code} and the reason on stderr`, async () => {
    const d = await fakeDaemon({ stdout: "", stderr, exit_code: code });
    try {
      const result = await Sandbox.attach("abc", { baseUrl: d.baseUrl }).runCommand("nosuch");
      assert.deepEqual(result, { stdout: "", stderr, exitCode: code });
    } finally { d.close(); }
  });
}

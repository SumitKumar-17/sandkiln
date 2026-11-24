// Live check against a real daemon: drives the built server over stdio like an
// MCP client would. Not part of `npm test` (needs a daemon and KVM).
//   SANDKILN_DAEMON_URL=http://127.0.0.1:7777 node test/live-smoke.mjs
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

// SANDKILN_MCP_ENTRY lets the same check run against an installed package (see packages/mcp/README.md, "Release check").
const entry = process.env.SANDKILN_MCP_ENTRY ?? fileURLToPath(new URL("../dist/index.js", import.meta.url));
const baseUrl = process.env.SANDKILN_DAEMON_URL ?? "http://127.0.0.1:7777";

function connect() {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [entry],
    env: { ...process.env, SANDKILN_DAEMON_URL: baseUrl },
    stderr: "inherit",
  });
  const client = new Client({ name: "live-smoke", version: "0.0.0" });
  return client.connect(transport).then(() => ({ client, transport }));
}

const step = (name, detail = "") => console.log(`ok  ${name}${detail ? "  " + detail : ""}`);
const call = async (client, name, args = {}) => {
  const r = await client.callTool({ name, arguments: args });
  const body = JSON.parse(r.content[0].text);
  return { isError: r.isError === true, body, structured: r.structuredContent };
};

const { client } = await connect();
step("initialize", `server=${client.getServerVersion().name}@${client.getServerVersion().version}`);

const tools = (await client.listTools()).tools.map((t) => t.name).sort();
for (const t of ["sandbox_create", "sandbox_exec", "sandbox_write_file", "sandbox_read_file", "sandbox_list_dir", "sandbox_snapshot", "sandbox_resume", "sandbox_fork", "sandbox_stop", "sandbox_list", "daemon_status"]) {
  assert.ok(tools.includes(t), `missing tool ${t}`);
}
step("tools/list", `${tools.length} tools: ${tools.join(", ")}`);
const resources = (await client.listResources()).resources.map((r) => r.uri);
step("resources/list", resources.join(", "));

const status = await call(client, "daemon_status");
assert.ok(!status.isError && status.body.version);
step("daemon_status", `version=${status.body.version} sandboxes=${status.body.counts.sandboxes} warnings=${status.body.warnings.length}`);

const created = await call(client, "sandbox_create", { tags: { owner: "mcp-live-smoke" }, egress: "deny_all" });
assert.ok(!created.isError, JSON.stringify(created.body));
const id = created.body.sandbox_id;
step("sandbox_create", `id=${id} stopped_on_exit=${created.body.stopped_on_exit}`);

const exec = await call(client, "sandbox_exec", { sandbox_id: id, command: "sh", args: ["-c", "echo out; echo err >&2; exit 3"] });
assert.equal(exec.body.exit_code, 3);
assert.equal(exec.body.stdout.trim(), "out");
assert.equal(exec.body.stderr.trim(), "err");
step("sandbox_exec", `exit_code=${exec.body.exit_code} stdout=${JSON.stringify(exec.body.stdout)} stderr=${JSON.stringify(exec.body.stderr)}`);

const big = await call(client, "sandbox_exec", { sandbox_id: id, command: "sh", args: ["-c", "head -c 200000 /dev/zero | tr '\\0' x"] });
assert.equal(big.body.stdout_truncated, true);
assert.ok(big.body.stdout.length < 20000);
step("sandbox_exec (huge output)", `total=${big.body.stdout_total_bytes} returned=${big.body.stdout.length} truncated=${big.body.stdout_truncated}`);

const wrote = await call(client, "sandbox_write_file", { sandbox_id: id, path: "/tmp/hello.txt", content: "héllo from mcp\n" });
assert.ok(!wrote.isError, JSON.stringify(wrote.body));
const read = await call(client, "sandbox_read_file", { sandbox_id: id, path: "/tmp/hello.txt" });
assert.equal(read.body.content, "héllo from mcp\n");
step("write_file + read_file", `bytes_written=${wrote.body.bytes_written} round-trip=${JSON.stringify(read.body.content)}`);

const ls = await call(client, "sandbox_list_dir", { sandbox_id: id, path: "/tmp" });
assert.ok(ls.body.entries.some((e) => e.name === "hello.txt"));
step("sandbox_list_dir", `/tmp has ${ls.body.total_entries} entries incl. hello.txt`);

const timeout = await call(client, "sandbox_exec", { sandbox_id: id, command: "sleep", args: ["30"], timeout_seconds: 2 });
assert.equal(timeout.isError, true);
assert.equal(timeout.body.error.kind, "timeout");
step("sandbox_exec timeout", `error.kind=${timeout.body.error.kind}`);

const missing = await call(client, "sandbox_exec", { sandbox_id: "does-not-exist", command: "true" });
assert.equal(missing.isError, true);
step("structured error", `kind=${missing.body.error.kind} status=${missing.body.error.status}`);

const badArgs = await client.callTool({ name: "sandbox_exec", arguments: { sandbox_id: id } });
assert.equal(badArgs.isError, true);
step("schema validation", "missing command rejected");

const snap = await call(client, "sandbox_snapshot", { sandbox_id: id });
assert.ok(!snap.isError, JSON.stringify(snap.body));
step("sandbox_snapshot", `snapshot_id=${snap.body.snapshot_id}`);

const resumed = await call(client, "sandbox_resume", { snapshot_id: snap.body.snapshot_id });
assert.ok(!resumed.isError, JSON.stringify(resumed.body));
const after = await call(client, "sandbox_read_file", { sandbox_id: resumed.body.sandbox_id, path: "/tmp/hello.txt" });
assert.equal(after.body.content, "héllo from mcp\n");
step("sandbox_resume", `new id=${resumed.body.sandbox_id}; file survived the snapshot`);

const list = await call(client, "sandbox_list", { tags: { owner: "mcp-live-smoke" } });
assert.ok(list.body.sandboxes.some((s) => s.id === resumed.body.sandbox_id && s.created_by_this_session));
step("sandbox_list", `${list.body.count} sandbox(es) tagged for this run`);

const res = await client.readResource({ uri: "sandkiln://sandboxes" });
assert.ok(JSON.parse(res.contents[0].text).some((s) => s.id === resumed.body.sandbox_id));
step("resources/read sandkiln://sandboxes", "contains the resumed sandbox");

const stopped = await call(client, "sandbox_stop", { sandbox_id: resumed.body.sandbox_id });
assert.deepEqual([stopped.body.kept, stopped.body.snapshot_id], [false, null]);
step("sandbox_stop", "keep=false, destroyed");

// Exit cleanup: leave a sandbox running, close the connection, check it is gone.
const orphan = await call(client, "sandbox_create", { tags: { owner: "mcp-live-smoke-exit" } });
assert.ok(!orphan.isError, JSON.stringify(orphan.body));
await client.close();
const poll = await connect();
let gone = false;
for (let i = 0; i < 20 && !gone; i++) {
  const l = await call(poll.client, "sandbox_list", { tags: { owner: "mcp-live-smoke-exit" } });
  gone = l.body.count === 0;
  if (!gone) await new Promise((r) => setTimeout(r, 500));
}
assert.ok(gone, "sandbox left running after the session closed");
step("cleanup on exit", `sandbox ${orphan.body.sandbox_id} stopped when the client disconnected`);
await poll.client.close();
console.log("ALL LIVE CHECKS PASSED");

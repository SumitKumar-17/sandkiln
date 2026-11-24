import { test } from "node:test";
import assert from "node:assert/strict";
import { ConfigError, DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_MAX_SANDBOXES, parseConfig } from "../dist/config.js";

test("defaults when the environment is empty", () => {
  assert.deepEqual(parseConfig({}), {
    baseUrl: undefined,
    authToken: undefined,
    maxOutputBytes: DEFAULT_MAX_OUTPUT_BYTES,
    maxSandboxes: DEFAULT_MAX_SANDBOXES,
    keepOnExit: false,
  });
});

test("reads daemon url, token and limits", () => {
  const c = parseConfig({
    SANDKILN_DAEMON_URL: "http://10.0.0.1:7777",
    SANDKILN_AUTH_TOKEN: "t",
    SANDKILN_MCP_MAX_OUTPUT_BYTES: "1000",
    SANDKILN_MCP_MAX_SANDBOXES: "2",
    SANDKILN_MCP_KEEP_ON_EXIT: "1",
  });
  assert.equal(c.baseUrl, "http://10.0.0.1:7777");
  assert.equal(c.authToken, "t");
  assert.equal(c.maxOutputBytes, 1000);
  assert.equal(c.maxSandboxes, 2);
  assert.equal(c.keepOnExit, true);
});

test("rejects malformed numbers and flags", () => {
  assert.throws(() => parseConfig({ SANDKILN_MCP_MAX_SANDBOXES: "0" }), ConfigError);
  assert.throws(() => parseConfig({ SANDKILN_MCP_MAX_OUTPUT_BYTES: "abc" }), ConfigError);
  assert.throws(() => parseConfig({ SANDKILN_MCP_KEEP_ON_EXIT: "maybe" }), ConfigError);
});

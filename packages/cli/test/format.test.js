import { test } from "node:test";
import assert from "node:assert/strict";
import { formatSandboxList, parseTag } from "../dist/format.js";

test("parseTag splits on the first = and accumulates into the previous object", () => {
  const acc = parseTag("env=prod", {});
  assert.deepEqual(acc, { env: "prod" });

  parseTag("owner=a=b", acc);
  assert.deepEqual(acc, { env: "prod", owner: "a=b" });
});

test("parseTag rejects a value with no =", () => {
  assert.throws(() => parseTag("noequals", {}), /--tag expects key=value, got: noequals/);
});

test("parseTag treats a leading = as an empty key", () => {
  const acc = parseTag("=value", {});
  assert.deepEqual(acc, { "": "value" });
});

test("formatSandboxList reports an empty list distinctly", () => {
  assert.equal(formatSandboxList([]), "no sandboxes\n");
});

test("formatSandboxList renders id, ISO timestamp, and comma-joined tags per line", () => {
  const sandboxes = [
    { id: "sb-1", createdAt: new Date("2026-01-01T00:00:00.000Z"), tags: { env: "prod", owner: "sumit" } },
    { id: "sb-2", createdAt: new Date("2026-01-02T00:00:00.000Z"), tags: {} },
  ];
  assert.equal(
    formatSandboxList(sandboxes),
    "sb-1  2026-01-01T00:00:00.000Z  env=prod,owner=sumit\n" + "sb-2  2026-01-02T00:00:00.000Z  \n",
  );
});

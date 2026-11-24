import { test } from "node:test";
import assert from "node:assert/strict";
import { SandboxSession } from "../dist/session.js";
import { ToolFailure } from "../dist/errors.js";

test("enforces the live sandbox limit and frees a slot on forget", () => {
  const s = new SandboxSession(2, async () => {});
  s.track("a");
  s.track("b");
  assert.throws(() => s.assertCanCreate(), (e) => e instanceof ToolFailure && e.kind === "limit_reached");
  s.forget("a");
  s.assertCanCreate();
  assert.equal(s.owns("a"), false);
  assert.equal(s.owns("b"), true);
});

test("cleanup stops every tracked sandbox with the keep flag", async () => {
  const calls = [];
  const s = new SandboxSession(5, async (id, keep) => void calls.push([id, keep]));
  s.track("a");
  s.track("b");
  assert.deepEqual(await s.cleanup(false), []);
  assert.deepEqual(calls.sort(), [["a", false], ["b", false]]);
  assert.deepEqual(s.ids(), []);
});

test("one failing stop does not block the others and is reported", async () => {
  const stopped = [];
  const s = new SandboxSession(5, async (id) => {
    if (id === "bad") throw new Error("nope");
    stopped.push(id);
  });
  for (const id of ["a", "bad", "c"]) s.track(id);
  assert.deepEqual(await s.cleanup(true), ["bad"]);
  assert.deepEqual(stopped.sort(), ["a", "c"]);
  assert.deepEqual(s.ids(), ["bad"]);
});

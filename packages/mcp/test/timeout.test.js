import { test } from "node:test";
import assert from "node:assert/strict";
import { withTimeout } from "../dist/timeout.js";

test("resolves with the work's value when it finishes in time", async () => {
  assert.equal(await withTimeout(Promise.resolve(7), 1000, () => new Error("late")), 7);
});

test("rejects with the supplied error when the work is too slow", async () => {
  const slow = new Promise((resolve) => setTimeout(resolve, 500));
  await assert.rejects(withTimeout(slow, 20, () => new Error("late")), /late/);
});

test("a rejection from abandoned work does not surface as unhandled", async () => {
  const slow = new Promise((_, reject) => setTimeout(() => reject(new Error("after")), 60));
  await assert.rejects(withTimeout(slow, 10, () => new Error("late")), /late/);
  await new Promise((r) => setTimeout(r, 100));
});

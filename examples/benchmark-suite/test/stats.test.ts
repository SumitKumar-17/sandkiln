import assert from "node:assert/strict";
import { test } from "node:test";
import { percentChange, percentile, summarize } from "../src/stats.ts";

test("percentile interpolates linearly", () => {
  const s = [10, 20, 30, 40];
  assert.equal(percentile(s, 0), 10);
  assert.equal(percentile(s, 100), 40);
  assert.equal(percentile(s, 50), 25);
  assert.equal(percentile(s, 95), 38.5);
});

test("percentile of one sample is that sample", () => {
  assert.equal(percentile([7], 99), 7);
});

test("percentile rejects empty input and bad p", () => {
  assert.throws(() => percentile([], 50), RangeError);
  assert.throws(() => percentile([1], 101), RangeError);
});

test("summarize sorts, and does not mutate its input", () => {
  const input = [5, 1, 3, 2, 4];
  const s = summarize(input);
  assert.deepEqual(input, [5, 1, 3, 2, 4]);
  assert.equal(s.n, 5);
  assert.equal(s.min, 1);
  assert.equal(s.max, 5);
  assert.equal(s.median, 3);
  assert.equal(s.mean, 3);
});

test("summarize rejects zero samples", () => {
  assert.throws(() => summarize([]), RangeError);
});

test("percentChange", () => {
  assert.equal(percentChange(100, 150), 50);
  assert.equal(percentChange(200, 100), -50);
  assert.equal(percentChange(0, 5), null);
});

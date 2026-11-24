import assert from "node:assert/strict";
import { test } from "node:test";
import { compareResults, hasRegression, renderCompare } from "../src/compare.ts";
import { fixture } from "./fixture.ts";

const opts = { thresholdPercent: 20, minDeltaMs: 0.5 };

test("a slower latency beyond the threshold is a regression", () => {
  const rows = compareResults(fixture({ create_call: [100] }), fixture({ create_call: [150] }), opts);
  assert.equal(rows[0]?.verdict, "regression");
  assert.equal(rows[0]?.change, 50);
  assert.ok(hasRegression(rows));
});

test("a faster latency is an improvement, not a regression", () => {
  const rows = compareResults(fixture({ create_call: [100] }), fixture({ create_call: [50] }), opts);
  assert.equal(rows[0]?.verdict, "improvement");
  assert.ok(!hasRegression(rows));
});

test("a change under the threshold is ok", () => {
  const rows = compareResults(fixture({ create_call: [100] }), fixture({ create_call: [110] }), opts);
  assert.equal(rows[0]?.verdict, "ok");
});

test("a large percent on a tiny absolute delta is ignored", () => {
  const rows = compareResults(fixture({ exec_roundtrip: [0.2] }), fixture({ exec_roundtrip: [0.3] }), opts);
  assert.equal(rows[0]?.verdict, "ok");
});

test("throughput regresses when it drops", () => {
  const down = compareResults(fixture({ p_throughput: [40] }), fixture({ p_throughput: [20] }), opts);
  assert.equal(down[0]?.verdict, "regression");
  const up = compareResults(fixture({ p_throughput: [40] }), fixture({ p_throughput: [80] }), opts);
  assert.equal(up[0]?.verdict, "improvement");
});

test("metrics present on one side only are reported, not flagged", () => {
  const rows = compareResults(fixture({ a: [1] }), fixture({ b: [1] }), opts);
  assert.deepEqual(rows.map((r) => r.verdict), ["only-before", "only-after"]);
  assert.ok(!hasRegression(rows));
});

test("renderCompare marks regressions", () => {
  const rows = compareResults(fixture({ create_call: [100] }), fixture({ create_call: [150] }), opts);
  assert.match(renderCompare(rows, opts), /REGRESSION/);
});

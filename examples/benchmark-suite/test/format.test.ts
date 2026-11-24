import assert from "node:assert/strict";
import { test } from "node:test";
import { formatPercent, formatValue, renderMarkdown, renderTable } from "../src/format.ts";
import { fixture } from "./fixture.ts";

test("formatValue picks a readable unit", () => {
  assert.equal(formatValue(0.245, "ms"), "245us");
  assert.equal(formatValue(18.456, "ms"), "18.46ms");
  assert.equal(formatValue(214.53, "ms"), "214.5ms");
  assert.equal(formatValue(41.26, "ops/s"), "41.3/s");
});

test("formatPercent signs and null", () => {
  assert.equal(formatPercent(12.34), "+12.3%");
  assert.equal(formatPercent(-5), "-5.0%");
  assert.equal(formatPercent(null), "n/a");
});

test("tables carry every metric and the sample count", () => {
  const r = fixture({ create_call: [10, 20, 30], exec_roundtrip: [0.2, 0.3] });
  const text = renderTable(r);
  assert.match(text, /create_call/);
  assert.match(text, /exec_roundtrip/);
  const md = renderMarkdown(r);
  assert.match(md, /\| create_call \| 20\.00ms \|/);
  assert.match(md, /## Environment/);
});

import { test } from "node:test";
import assert from "node:assert/strict";
import { decodeFile, truncateText } from "../dist/truncate.js";

test("short text passes through untouched", () => {
  const r = truncateText("hello", 100);
  assert.deepEqual(r, { text: "hello", truncated: false, totalBytes: 5, omittedBytes: 0 });
});

test("long text keeps head and tail with an omission note", () => {
  const text = "A".repeat(1000) + "MIDDLE" + "Z".repeat(1000);
  const r = truncateText(text, 100);
  assert.equal(r.truncated, true);
  assert.equal(r.totalBytes, 2006);
  assert.ok(r.text.startsWith("A".repeat(50)));
  assert.ok(r.text.endsWith("Z".repeat(50)));
  assert.match(r.text, /\[\.\.\. \d+ bytes omitted of 2006 total \.\.\.\]/);
  assert.ok(!r.text.includes("MIDDLE"));
  assert.equal(r.omittedBytes, 2006 - 100);
});

test("a cut never splits a multi-byte character", () => {
  // Each emoji is 4 bytes; odd budgets would land mid-character.
  const text = "😀".repeat(100);
  for (const budget of [7, 10, 13, 50, 101]) {
    const r = truncateText(text, budget);
    assert.ok(!r.text.includes("�"), `replacement char at budget ${budget}`);
  }
});

test("decodeFile returns utf8 text when the bytes are text", () => {
  const f = decodeFile(new TextEncoder().encode("héllo\n"), 1000);
  assert.equal(f.encoding, "utf8");
  assert.equal(f.content, "héllo\n");
  assert.equal(f.truncated, false);
  assert.equal(f.returnedBytes, f.totalBytes);
});

test("decodeFile truncates text on a character boundary", () => {
  const f = decodeFile(new TextEncoder().encode("é".repeat(10)), 5);
  assert.equal(f.encoding, "utf8");
  assert.equal(f.truncated, true);
  assert.equal(f.totalBytes, 20);
  assert.equal(f.content, "éé");
  assert.equal(f.returnedBytes, 4);
});

test("decodeFile falls back to base64 for NUL bytes and invalid UTF-8", () => {
  const nul = decodeFile(Uint8Array.from([1, 0, 2]), 100);
  assert.equal(nul.encoding, "base64");
  assert.equal(nul.content, Buffer.from([1, 0, 2]).toString("base64"));
  const bad = decodeFile(Uint8Array.from([0xff, 0xfe, 0x41]), 100);
  assert.equal(bad.encoding, "base64");
});

test("decodeFile cuts binary at exactly maxBytes", () => {
  const f = decodeFile(Uint8Array.from([0, 1, 2, 3, 4, 5]), 4);
  assert.equal(f.encoding, "base64");
  assert.equal(f.returnedBytes, 4);
  assert.equal(f.truncated, true);
});

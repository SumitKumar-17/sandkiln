import assert from "node:assert/strict";
import { test } from "node:test";
import { parseCli } from "../src/args.ts";
import { isLoopback, parseCpuModel } from "../src/env.ts";

test("run defaults come from env then built-ins", () => {
  const a = parseCli([], { SANDKILN_DAEMON_URL: "http://box:7777", SANDKILN_AUTH_TOKEN: "t" });
  assert.equal(a.command, "run");
  if (a.command !== "run") return;
  assert.equal(a.baseUrl, "http://box:7777");
  assert.equal(a.authToken, "t");
  assert.equal(a.iterations, 20);
  assert.equal(a.concurrency, 8);
});

test("flags override env", () => {
  const a = parseCli(["--base-url", "http://x:1", "--iterations", "5", "--only", "exec,create"], { SANDKILN_DAEMON_URL: "http://env" });
  if (a.command !== "run") throw new Error("expected run");
  assert.equal(a.baseUrl, "http://x:1");
  assert.equal(a.iterations, 5);
  assert.deepEqual(a.only, ["exec", "create"]);
});

test("invalid numbers are rejected", () => {
  assert.throws(() => parseCli(["--iterations", "0"], {}), /positive integer/);
  assert.throws(() => parseCli(["--concurrency", "abc"], {}), /positive integer/);
});

test("compare needs two files and parses thresholds", () => {
  assert.throws(() => parseCli(["compare", "a.json"], {}), /two files/);
  const a = parseCli(["compare", "a.json", "b.json", "--threshold", "10"], {});
  assert.deepEqual(a, { command: "compare", before: "a.json", after: "b.json", thresholdPercent: 10, minDeltaMs: 0.5 });
});

test("unknown command is rejected", () => {
  assert.throws(() => parseCli(["bogus"], {}), /unknown command/);
});

test("isLoopback", () => {
  assert.ok(isLoopback("http://127.0.0.1:7777"));
  assert.ok(isLoopback("http://localhost:7777"));
  assert.ok(!isLoopback("http://10.5.31.157:7777"));
});

test("parseCpuModel reads the model and thread count", () => {
  const info = "processor\t: 0\nmodel name\t: Test CPU @ 3GHz\nprocessor\t: 1\nmodel name\t: Test CPU @ 3GHz\n";
  assert.equal(parseCpuModel(info), "Test CPU @ 3GHz (2 threads)");
  assert.equal(parseCpuModel("nothing"), null);
});

import { test } from "node:test";
import assert from "node:assert/strict";
import { Sandbox } from "../dist/index.js";

function sandbox(options = {}) {
  return Sandbox.attach("sbx-1", { baseUrl: "http://127.0.0.1:7777", ...options });
}

test("previewUrl defaults to the root path with no query string when no auth token is set", () => {
  assert.equal(sandbox().previewUrl(3000), "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/");
});

test("previewUrl adds a leading slash to a path that's missing one", () => {
  assert.equal(sandbox().previewUrl(3000, { path: "api/health" }), "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/api/health");
});

test("previewUrl preserves a path that already has a leading slash", () => {
  assert.equal(sandbox().previewUrl(3000, { path: "/api/health" }), "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/api/health");
});

test("previewUrl appends the auth token as a ?token= query parameter when one is configured", () => {
  const url = sandbox({ authToken: "secret123" }).previewUrl(3000);
  assert.equal(url, "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/?token=secret123");
});

test("previewUrl combines a custom path with the auth token query parameter", () => {
  const url = sandbox({ authToken: "secret123" }).previewUrl(3000, { path: "/app" });
  assert.equal(url, "http://127.0.0.1:7777/sandboxes/sbx-1/preview/3000/app?token=secret123");
});

test("previewUrl URI-encodes the sandbox id", () => {
  const url = Sandbox.attach("weird id/with:chars", { baseUrl: "http://127.0.0.1:7777" }).previewUrl(3000);
  assert.equal(url, "http://127.0.0.1:7777/sandboxes/weird%20id%2Fwith%3Achars/preview/3000/");
});

test("previewUrl rejects an out-of-range or non-integer port", () => {
  assert.throws(() => sandbox().previewUrl(0), RangeError);
  assert.throws(() => sandbox().previewUrl(65536), RangeError);
  assert.throws(() => sandbox().previewUrl(3.5), RangeError);
  assert.throws(() => sandbox().previewUrl(-1), RangeError);
});

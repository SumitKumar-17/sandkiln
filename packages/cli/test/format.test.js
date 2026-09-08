import { test } from "node:test";
import assert from "node:assert/strict";
import { InvalidArgumentError } from "commander";
import { formatDriveList, formatImageList, formatSandboxList, formatSnapshotList, parseDriveAttachment, parseTag } from "../dist/format.js";

test("parseTag splits on the first = and accumulates into the previous object", () => {
  const acc = parseTag("env=prod", {});
  assert.deepEqual(acc, { env: "prod" });

  parseTag("owner=a=b", acc);
  assert.deepEqual(acc, { env: "prod", owner: "a=b" });
});

test("parseTag rejects a value with no = with a commander InvalidArgumentError", () => {
  // Must be commander's InvalidArgumentError, not a plain Error — that's
  // the type commander recognizes to print a clean `error: ...` message
  // instead of letting the exception escape as a raw stack trace.
  assert.throws(() => parseTag("noequals", {}), (err) => {
    assert.ok(err instanceof InvalidArgumentError);
    assert.match(err.message, /--tag expects key=value, got: noequals/);
    return true;
  });
});

test("parseTag treats a leading = as an empty key", () => {
  const acc = parseTag("=value", {});
  assert.deepEqual(acc, { "": "value" });
});

test("formatSandboxList reports an empty list distinctly", () => {
  assert.equal(formatSandboxList([]), "no sandboxes\n");
});

test("formatSandboxList renders id, ISO timestamp, name, and comma-joined tags per line", () => {
  const sandboxes = [
    {
      id: "sb-1",
      createdAt: new Date("2026-01-01T00:00:00.000Z"),
      tags: { env: "prod", owner: "sumit" },
      name: "web-server",
    },
    { id: "sb-2", createdAt: new Date("2026-01-02T00:00:00.000Z"), tags: {} },
  ];
  assert.equal(
    formatSandboxList(sandboxes),
    "sb-1  2026-01-01T00:00:00.000Z  web-server  env=prod,owner=sumit\n" + "sb-2  2026-01-02T00:00:00.000Z  -  \n",
  );
});

test("formatSandboxList shows a dash for an unnamed sandbox", () => {
  const sandboxes = [{ id: "sb-3", createdAt: new Date("2026-01-03T00:00:00.000Z"), tags: {} }];
  assert.equal(formatSandboxList(sandboxes), "sb-3  2026-01-03T00:00:00.000Z  -  \n");
});

test("formatSnapshotList reports an empty list distinctly", () => {
  assert.equal(formatSnapshotList([]), "no snapshots\n");
});

test("formatSnapshotList renders id, source sandbox id, timestamp, and tags per line", () => {
  const snapshots = [
    {
      id: "snap-1",
      sourceSandboxId: "sb-1",
      createdAt: new Date("2026-01-01T00:00:00.000Z"),
      tags: { env: "prod" },
      forkedInto: null,
    },
  ];
  assert.equal(formatSnapshotList(snapshots), "snap-1  source=sb-1  2026-01-01T00:00:00.000Z  env=prod\n");
});

test("formatSnapshotList appends forked_into only when a live fork exists", () => {
  const snapshots = [
    { id: "snap-1", sourceSandboxId: "sb-1", createdAt: new Date("2026-01-01T00:00:00.000Z"), tags: {}, forkedInto: "sb-2" },
  ];
  assert.equal(formatSnapshotList(snapshots), "snap-1  source=sb-1  2026-01-01T00:00:00.000Z    forked_into=sb-2\n");
});

test("formatImageList reports an empty list distinctly", () => {
  assert.equal(formatImageList([]), "no images\n");
});

test("formatImageList renders id, size, ISO timestamp, and in-use holder (or 'not in use') per line", () => {
  const images = [
    {
      id: "python-3.12-custom",
      sizeMib: 2048,
      createdAt: new Date("2026-01-01T00:00:00.000Z"),
      inUseBy: "sandbox sb-1",
      guestAgentVerified: false,
      verificationHint: "...",
    },
    {
      id: "node-lts-custom",
      sizeMib: 1024,
      createdAt: new Date("2026-01-02T00:00:00.000Z"),
      inUseBy: null,
      guestAgentVerified: false,
      verificationHint: "...",
    },
  ];
  assert.equal(
    formatImageList(images),
    "python-3.12-custom  2048MiB  2026-01-01T00:00:00.000Z  sandbox sb-1\n" +
      "node-lts-custom  1024MiB  2026-01-02T00:00:00.000Z  not in use\n",
  );
});

test("parseDriveAttachment accumulates a bare id as a read-write attachment", () => {
  const acc = parseDriveAttachment("drv-1", []);
  assert.deepEqual(acc, [{ id: "drv-1" }]);

  parseDriveAttachment("drv-2", acc);
  assert.deepEqual(acc, [{ id: "drv-1" }, { id: "drv-2" }]);
});

test("parseDriveAttachment treats a :ro suffix as a read-only attachment", () => {
  const acc = parseDriveAttachment("drv-1:ro", []);
  assert.deepEqual(acc, [{ id: "drv-1", readOnly: true }]);
});

test("parseDriveAttachment rejects any suffix other than :ro", () => {
  assert.throws(() => parseDriveAttachment("drv-1:rw", []), (err) => {
    assert.ok(err instanceof InvalidArgumentError);
    assert.match(err.message, /--drive expects <id> or <id>:ro, got: drv-1:rw/);
    return true;
  });
});

test("formatDriveList reports an empty list distinctly", () => {
  assert.equal(formatDriveList([]), "no drives\n");
});

test("formatDriveList renders id, size, timestamp, and comma-joined holders per line", () => {
  const drives = [
    {
      id: "drv-1",
      sizeMib: 64,
      createdAt: new Date("2026-01-01T00:00:00.000Z"),
      attachedTo: [{ holder: "sandbox sb-1", readOnly: false }],
    },
    {
      id: "drv-2",
      sizeMib: 128,
      createdAt: new Date("2026-01-02T00:00:00.000Z"),
      attachedTo: [],
    },
    {
      id: "drv-3",
      sizeMib: 256,
      createdAt: new Date("2026-01-03T00:00:00.000Z"),
      attachedTo: [
        { holder: "sandbox sb-2", readOnly: true },
        { holder: "sandbox sb-3", readOnly: true },
      ],
    },
  ];
  assert.equal(
    formatDriveList(drives),
    "drv-1  64MiB  2026-01-01T00:00:00.000Z  sandbox sb-1\n" +
      "drv-2  128MiB  2026-01-02T00:00:00.000Z  not attached\n" +
      "drv-3  256MiB  2026-01-03T00:00:00.000Z  sandbox sb-2:ro,sandbox sb-3:ro\n",
  );
});

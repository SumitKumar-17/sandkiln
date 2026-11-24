import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, writeFile, readFile, symlink, lstat, readlink, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { downloadDir, uploadDir } from "../dist/index.js";

/** In-memory stand-in for a sandbox filesystem. */
function fakeSandbox(initial = {}) {
  const files = new Map(Object.entries(initial.files ?? {}).map(([p, v]) => [p, { content: Buffer.from(v), mode: 0o644 }]));
  const dirs = new Set(initial.dirs ?? ["/"]);
  const links = new Map(Object.entries(initial.links ?? {}));
  return {
    files, dirs, links,
    async readFile(p) { return files.get(p).content; },
    async writeFile(p, c) { files.set(p, { content: Buffer.from(c), mode: 0o644 }); },
    async mkdir(p) { dirs.add(p); },
    async chmod(p, m) { files.get(p).mode = m; },
    async symlink(t, l) { links.set(l, t); },
    async readlink(p) { return links.get(p); },
    async listDir(dir) {
      const prefix = dir.endsWith("/") ? dir : dir + "/";
      const names = new Map();
      for (const p of files.keys()) if (p.startsWith(prefix) && !p.slice(prefix.length).includes("/")) names.set(p.slice(prefix.length), { isDir: false, isSymlink: false, mode: files.get(p).mode });
      for (const d of dirs) if (d.startsWith(prefix) && d !== dir && !d.slice(prefix.length).includes("/")) names.set(d.slice(prefix.length), { isDir: true, isSymlink: false, mode: 0o755 });
      for (const l of links.keys()) if (l.startsWith(prefix) && !l.slice(prefix.length).includes("/")) names.set(l.slice(prefix.length), { isDir: false, isSymlink: true, mode: 0o777 });
      return [...names].map(([name, e]) => ({ name, size: 0, mtime: new Date(0), ...e }));
    },
  };
}

async function tmp() { return mkdtemp(path.join(os.tmpdir(), "sandkiln-transfer-")); }

test("uploadDir copies files, modes, nested dirs and symlinks and honors exclude", async () => {
  const local = await tmp();
  await mkdir(path.join(local, "src/deep"), { recursive: true });
  await mkdir(path.join(local, "node_modules"), { recursive: true });
  await writeFile(path.join(local, "run.sh"), "echo hi", { mode: 0o755 });
  await writeFile(path.join(local, "src/deep/a.txt"), "A");
  await writeFile(path.join(local, "node_modules/skip.js"), "x");
  await symlink("run.sh", path.join(local, "link"));
  const sbx = fakeSandbox();
  const r = await uploadDir(sbx, local, "/work", { exclude: (rel) => rel.startsWith("node_modules") });
  assert.equal(r.files, 2);
  assert.equal(sbx.files.get("/work/run.sh").mode, 0o755);
  assert.equal(sbx.files.get("/work/src/deep/a.txt").content.toString(), "A");
  assert.equal(sbx.links.get("/work/link"), "run.sh");
  assert.ok(!sbx.files.has("/work/node_modules/skip.js"));
  assert.deepEqual(r.skipped, ["node_modules"]);
  await rm(local, { recursive: true });
});

test("downloadDir round-trips a tree and keeps modes", async () => {
  const sbx = fakeSandbox({ dirs: ["/", "/out", "/out/sub"], files: { "/out/a.txt": "hello", "/out/sub/b.txt": "world" }, links: { "/out/ok": "a.txt" } });
  sbx.files.get("/out/a.txt").mode = 0o600;
  const local = await tmp();
  const r = await downloadDir(sbx, "/out", local);
  assert.equal(r.files, 2);
  assert.equal(await readFile(path.join(local, "sub/b.txt"), "utf8"), "world");
  assert.equal((await lstat(path.join(local, "a.txt"))).mode & 0o777, 0o600);
  assert.equal(await readlink(path.join(local, "ok")), "a.txt");
  await rm(local, { recursive: true });
});

test("downloadDir refuses a hostile guest: traversal names throw, escaping symlinks are skipped", async () => {
  const local = await tmp();
  const evilName = fakeSandbox({ dirs: ["/", "/out"] });
  evilName.listDir = async () => [{ name: "../escape", isDir: false, isSymlink: false, size: 0, mode: 0o644, mtime: new Date(0) }];
  await assert.rejects(() => downloadDir(evilName, "/out", local), /unsafe entry name/);

  const evilLinks = fakeSandbox({ dirs: ["/", "/out"], links: { "/out/abs": "/etc/passwd", "/out/up": "../../outside", "/out/fine": "x" } });
  const r = await downloadDir(evilLinks, "/out", local);
  assert.deepEqual(r.skipped.sort(), ["abs", "up"]);
  assert.equal(r.symlinks, 1);
  await assert.rejects(() => lstat(path.join(local, "abs")));
  await rm(local, { recursive: true });
});

test("downloadDir refuses a listing that names an entry twice", async () => {
  const local = await tmp();
  const sbx = fakeSandbox({ dirs: ["/", "/out"], files: { "/out/a": "x" } });
  const real = sbx.listDir;
  sbx.listDir = async (dir) => { const e = await real(dir); return dir === "/out" ? [...e, { ...e[0] }] : e; };
  await assert.rejects(() => downloadDir(sbx, "/out", local), /duplicate entry name/);
  await rm(local, { recursive: true });
});

test("downloadDir judges a chain of symlinks by where it really resolves", async () => {
  const local = await tmp();
  // s -> .. stays inside the destination (it points at the root); t -> s/.. looks inside
  // lexically but really resolves one level above the destination.
  const sbx = fakeSandbox({ dirs: ["/", "/out", "/out/x"], links: { "/out/x/s": "..", "/out/x/t": "s/.." } });
  const r = await downloadDir(sbx, "/out", local);
  assert.deepEqual(r.skipped, ["x/t"]);
  assert.equal(r.symlinks, 1);
  await assert.rejects(() => lstat(path.join(local, "x/t")));
  await rm(local, { recursive: true });
});

test("downloadDir never writes through a symlink already in the destination", async () => {
  const local = await tmp();
  const outside = await tmp();
  await writeFile(path.join(outside, "victim"), "keep");
  await symlink(path.join(outside, "victim"), path.join(local, "a"));
  await mkdir(path.join(local, "d0"));
  await symlink(outside, path.join(local, "d"));
  const sbx = fakeSandbox({ dirs: ["/", "/out", "/out/d"], files: { "/out/a": "overwritten", "/out/d/f": "nope" } });
  await assert.rejects(() => downloadDir(sbx, "/out", local));
  assert.equal(await readFile(path.join(outside, "victim"), "utf8"), "keep");
  await assert.rejects(() => lstat(path.join(outside, "f")));
  await rm(local, { recursive: true }); await rm(outside, { recursive: true });
});

#!/usr/bin/env node
// A stand-in for sandkilnd that tests the sandkiln-run action's wiring on a
// GitHub-hosted runner, where there is no KVM. It speaks only the routes the
// action's script reaches through the kiln CLI, and it is NOT a microVM:
// commands run as ordinary processes of the runner, in a per-sandbox temp
// directory. See README.md in this directory for what that does and does not prove.
//
// Environment: STUB_PORT (default 7777), STUB_TOKEN (bearer token to require;
// no auth when unset), STUB_LOG (JSON-lines file recording every request; the
// token itself is never recorded, only whether it matched).

import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { appendFileSync, chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve, sep } from "node:path";
import { WebSocketServer } from "ws";

const PORT = Number(process.env.STUB_PORT ?? 7777);
const TOKEN = process.env.STUB_TOKEN ?? "";
const LOG = process.env.STUB_LOG ?? "";

/** id -> { root, sessions: Map<sessionId, session> } */
const sandboxes = new Map();

function record(entry) {
  if (LOG) appendFileSync(LOG, `${JSON.stringify({ at: new Date().toISOString(), ...entry })}\n`);
}

/** The guest paths the action uses are mapped into the sandbox's directory.
 * The file API maps every absolute path; commands only get the two prefixes
 * the action's script ever names, in one pass so a mapped root is not re-mapped. */
export function mapCommandText(root, text) {
  return text.replace(/\/workspace\b|\/tmp\/sandkiln-/g, (m) => join(root, m));
}

function hostPath(root, guestPath) {
  const full = resolve(root, `.${resolve("/", guestPath)}`);
  if (full !== root && !full.startsWith(root + sep)) throw httpError(400, "path escapes the sandbox");
  return full;
}

function httpError(status, message, extra = {}) {
  return Object.assign(new Error(message), { status, extra });
}

/** Auth outcome for a request: "none-required", "ok", "missing" or "bad". */
function authState(header, queryToken) {
  if (!TOKEN) return "none-required";
  const given = header?.startsWith("Bearer ") ? header.slice(7) : queryToken;
  if (!given) return "missing";
  return given === TOKEN ? "ok" : "bad";
}

function sandboxEnv(root) {
  return { PATH: process.env.PATH, HOME: root, LANG: "C.UTF-8" };
}

function runCommand(sb, command, args) {
  return spawn(mapCommandText(sb.root, command), args.map((a) => mapCommandText(sb.root, a)), {
    cwd: sb.root,
    env: sandboxEnv(sb.root),
    detached: true, // own process group, so a limit or a destroy can kill the whole tree
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function killGroup(child, signal = "SIGKILL") {
  try {
    process.kill(-child.pid, signal);
  } catch {
    // already gone
  }
}

function exitCodeOf(code, signal) {
  return code ?? (signal === "SIGKILL" ? 137 : 143);
}

async function readJson(req) {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const text = Buffer.concat(chunks).toString("utf8");
  return text ? JSON.parse(text) : {};
}

function send(res, status, body) {
  if (body === undefined) {
    res.writeHead(status).end();
    return status;
  }
  res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(body));
  return status;
}

function destroy(id) {
  const sb = sandboxes.get(id);
  for (const s of sb.sessions.values()) if (!s.done) killGroup(s.child);
  rmSync(sb.root, { recursive: true, force: true });
  sandboxes.delete(id);
}

function summarise(route, body) {
  switch (route) {
    case "create":
      return { tags: body.tags ?? {}, egress: body.egress ?? null, image: body.image_id ?? null };
    case "exec":
    case "exec-stream":
      return { command: body.command, args: body.args, timeout_secs: body.timeout_secs ?? null };
    case "write-file":
      return { path: body.path, bytes: Buffer.from(body.content_base64 ?? "", "base64").length };
    default:
      return body.path === undefined ? {} : { path: body.path };
  }
}

async function handleApi(req, res, url, entry) {
  const parts = url.pathname.split("/").filter(Boolean).map(decodeURIComponent);
  const method = req.method;

  if (method === "GET" && url.pathname === "/healthz") return send(res, 200, { ok: true });
  entry.auth = authState(req.headers.authorization, null);
  if (entry.auth === "missing" || entry.auth === "bad") return send(res, 401, { error: "unauthorized" });

  if (method === "POST" && url.pathname === "/sandboxes") {
    const body = await readJson(req);
    entry.body = summarise("create", body);
    const id = `stub-${randomBytes(4).toString("hex")}`;
    const root = mkdtempSync(join(tmpdir(), "sandkiln-stub-sbx-"));
    mkdirSync(join(root, "tmp"));
    sandboxes.set(id, { root, sessions: new Map() });
    entry.sandbox = id;
    return send(res, 200, { id });
  }

  if (parts[0] !== "sandboxes" || parts.length < 2) return send(res, 404, { error: "no such route" });
  const id = parts[1];
  entry.sandbox = id;
  const sb = sandboxes.get(id);
  if (!sb) return send(res, 404, { error: `sandbox ${id} not found` });

  if (method === "DELETE" && parts.length === 2) {
    entry.keep = url.searchParams.get("keep");
    destroy(id);
    if (url.searchParams.get("keep") === "false") return send(res, 204);
    return send(res, 200, { kept: true, snapshot_id: `snap-${id}` });
  }
  if (method === "GET" && parts[2] === "exec-stream") {
    const sessions = [...sb.sessions.entries()].map(([sid, s]) => ({
      id: sid, command: s.command, args: s.args, started_at_unix: s.started, exit_code: s.exitCode ?? null,
    }));
    return send(res, 200, { sessions });
  }
  if (method !== "POST" || parts.length !== 3) return send(res, 404, { error: "no such route" });

  const body = await readJson(req);
  const route = parts[2];
  entry.body = summarise(route, body);
  switch (route) {
    case "exec":
      return execOnce(res, sb, body);
    case "exec-stream":
      return startSession(res, sb, body);
    case "write-file": {
      const file = hostPath(sb.root, body.path);
      mkdirSync(dirname(file), { recursive: true });
      writeFileSync(file, Buffer.from(body.content_base64, "base64"));
      return send(res, 204);
    }
    case "read-file":
      try {
        return send(res, 200, { content_base64: readFileSync(hostPath(sb.root, body.path)).toString("base64") });
      } catch (e) {
        if (e.status) throw e;
        return send(res, 404, { error: `${body.path}: no such file` });
      }
    case "mkdir":
      mkdirSync(hostPath(sb.root, body.path), { recursive: body.parents === true });
      return send(res, 204);
    case "chmod":
      chmodSync(hostPath(sb.root, body.path), body.mode);
      return send(res, 204);
    default:
      return send(res, 404, { error: "no such route" });
  }
}

function execOnce(res, sb, { command, args = [], timeout_secs: limit }) {
  return new Promise((done) => {
    const child = runCommand(sb, command, args);
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    child.stdout.on("data", (d) => (stdout += d));
    child.stderr.on("data", (d) => (stderr += d));
    const timer = limit ? setTimeout(() => { timedOut = true; killGroup(child); }, limit * 1000) : undefined;
    child.on("error", (e) => done(send(res, 400, { error: `could not start ${command}: ${e.message}` })));
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      if (timedOut) {
        done(send(res, 504, { error: `command timed out after ${limit}s`, timed_out: true, timeout_secs: limit, stdout, stderr }));
      } else {
        done(send(res, 200, { stdout, stderr, exit_code: exitCodeOf(code, signal) }));
      }
    });
  });
}

function startSession(res, sb, { command, args = [] }) {
  const sid = `sess-${randomBytes(3).toString("hex")}`;
  const child = runCommand(sb, command, args);
  const session = { child, command, args, started: Math.floor(Date.now() / 1000), chunks: [], watchers: new Set(), done: false, exitCode: undefined };
  const push = (frame) => {
    session.chunks.push(frame);
    for (const ws of session.watchers) ws.send(frame.data, { binary: frame.binary });
  };
  child.stdout.on("data", (d) => push({ binary: true, data: d }));
  child.stderr.on("data", (d) => push({ binary: true, data: d }));
  child.on("error", (e) => push({ binary: true, data: Buffer.from(`${e.message}\n`) }));
  child.on("close", (code, signal) => {
    session.done = true;
    session.exitCode = exitCodeOf(code, signal);
    push({ binary: false, data: `[process exited with code ${session.exitCode}]` });
    for (const ws of session.watchers) ws.close();
  });
  sb.sessions.set(sid, session);
  return send(res, 200, { id: sid });
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url, "http://stub");
  const entry = { method: req.method, path: url.pathname };
  let status = 500;
  try {
    status = await handleApi(req, res, url, entry);
  } catch (e) {
    status = send(res, e.status ?? 500, { error: e.message, ...(e.extra ?? {}) });
  }
  record({ ...entry, status });
});

// Log attach: replay everything captured so far, then live-tail, then close.
const wss = new WebSocketServer({ noServer: true });
server.on("upgrade", (req, socket, head) => {
  const url = new URL(req.url, "http://stub");
  const m = /^\/sandboxes\/([^/]+)\/exec-stream\/([^/]+)\/logs$/.exec(url.pathname);
  const entry = { method: "WS", path: url.pathname };
  entry.auth = authState(req.headers.authorization, url.searchParams.get("token"));
  const session = m && sandboxes.get(decodeURIComponent(m[1]))?.sessions.get(decodeURIComponent(m[2]));
  if (m) entry.sandbox = decodeURIComponent(m[1]);
  if (entry.auth === "missing" || entry.auth === "bad" || !session) {
    const status = entry.auth === "missing" || entry.auth === "bad" ? 401 : 404;
    record({ ...entry, status });
    socket.end(`HTTP/1.1 ${status} Rejected\r\nConnection: close\r\n\r\n`);
    return;
  }
  record({ ...entry, status: 101 });
  wss.handleUpgrade(req, socket, head, (ws) => {
    for (const f of session.chunks) ws.send(f.data, { binary: f.binary });
    if (session.done) return ws.close();
    session.watchers.add(ws);
    ws.on("close", () => session.watchers.delete(ws));
  });
});

server.listen(PORT, "127.0.0.1", () => console.log(`stub sandkilnd listening on 127.0.0.1:${PORT}`));
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => {
    for (const id of [...sandboxes.keys()]) destroy(id);
    process.exit(0);
  });
}

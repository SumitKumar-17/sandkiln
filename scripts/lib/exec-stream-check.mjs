#!/usr/bin/env node
// Helper for scripts/integration-tests/23-exec-stream-logs.sh -- same
// reasoning as pty-check.mjs: the bash+curl harness has no native way to
// drive a WebSocket. This starts a background exec-stream session (a
// real REST POST, not a WebSocket) and attaches to its logs *while it's
// still running*, to prove replay-then-live-tail actually works end to
// end -- not just that the kernel-level mechanics do. Prints what it saw
// to stdout/stderr and exits 0/1; the bash side reads exit status, not
// structured output.

const [base, sandboxId, token] = process.argv.slice(2);
if (!base || !sandboxId) {
  console.error("usage: exec-stream-check.mjs <base-url> <sandbox-id> [token]");
  process.exit(2);
}

const headers = { "content-type": "application/json", ...(token ? { authorization: `Bearer ${token}` } : {}) };

async function startSession() {
  const res = await fetch(`${base}/sandboxes/${encodeURIComponent(sandboxId)}/exec-stream`, {
    method: "POST",
    headers,
    body: JSON.stringify({ command: "sh", args: ["-c", "for i in 1 2 3 4; do echo exec-stream-line-$i; sleep 1; done"] }),
  });
  if (!res.ok) throw new Error(`start exec-stream failed: ${res.status} ${await res.text()}`);
  return (await res.json()).id;
}

function attach(sessionId) {
  const wsBase = base.replace(/^http/, "ws");
  const url = new URL(`${wsBase}/sandboxes/${encodeURIComponent(sandboxId)}/exec-stream/${encodeURIComponent(sessionId)}/logs`);
  if (token) url.searchParams.set("token", token);

  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    let received = "";
    const timeout = setTimeout(() => reject(new Error(`timed out attaching (received so far: ${JSON.stringify(received)})`)), 10000);
    ws.addEventListener("message", (event) => {
      received += event.data instanceof ArrayBuffer ? Buffer.from(event.data).toString("utf8") : String(event.data);
    });
    ws.addEventListener("close", () => {
      clearTimeout(timeout);
      resolve(received);
    });
    ws.addEventListener("error", (event) => {
      clearTimeout(timeout);
      reject(new Error(`WebSocket error: ${event.message ?? event}`));
    });
  });
}

try {
  const sessionId = await startSession();

  // Attach immediately, well before the ~4s command finishes -- this is
  // the actual claim being tested: replay (nothing yet) then a live tail
  // of each line as it's produced, not just a bare dump after the fact.
  const live = await attach(sessionId);
  const expectedLines = [1, 2, 3, 4].map((i) => `exec-stream-line-${i}`);
  const missing = expectedLines.filter((line) => !live.includes(line));
  if (missing.length > 0) {
    console.error(`live attach missing expected lines ${JSON.stringify(missing)} -- got: ${JSON.stringify(live)}`);
    process.exit(1);
  }
  if (!live.includes("[process exited with code 0]")) {
    console.error(`live attach never saw the exit notice -- got: ${JSON.stringify(live)}`);
    process.exit(1);
  }

  // Reattach *after* the process has already finished -- proves the
  // replay buffer survives independent of any one connection, the whole
  // point of this being a background session rather than a streaming
  // variant of a request-scoped exec.
  const replay = await attach(sessionId);
  const stillMissing = expectedLines.filter((line) => !replay.includes(line));
  if (stillMissing.length > 0) {
    console.error(`reattach-after-finish missing expected lines ${JSON.stringify(stillMissing)} -- got: ${JSON.stringify(replay)}`);
    process.exit(1);
  }

  console.log("exec-stream replay-then-live-tail and reattach-after-finish both verified");
  process.exit(0);
} catch (e) {
  console.error(e.message ?? e);
  process.exit(1);
}

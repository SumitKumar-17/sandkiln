#!/usr/bin/env node
// Helper for scripts/integration-tests/17-pty.sh -- the bash harness has no
// good native way to drive a WebSocket, so this does the one thing that
// file needs: open a real PTY session against a running daemon, either
// round-trip a command through it and wait for the shell to exit cleanly
// (default), or open one and hang up immediately without telling the
// remote shell to exit (--disconnect-only, used to check the guest agent's
// SIGHUP-on-hangup cleanup from `scripts/integration-tests/17-pty.sh`).
// Prints what it saw to stdout/stderr and exits 0/1 -- the bash side reads
// exit status, not structured output.

const args = process.argv.slice(2);
const disconnectOnly = args.includes("--disconnect-only");
const positional = args.filter((a) => a !== "--disconnect-only");
const [base, sandboxId, token] = positional;

if (!base || !sandboxId) {
  console.error("usage: pty-check.mjs [--disconnect-only] <base-url> <sandbox-id> [token]");
  process.exit(2);
}

const wsBase = base.replace(/^http/, "ws");
const url = new URL(`${wsBase}/sandboxes/${encodeURIComponent(sandboxId)}/pty`);
if (token) url.searchParams.set("token", token);

const ws = new WebSocket(url);
ws.binaryType = "arraybuffer";

const MARKER = "SANDKILN_PTY_INTEGRATION_TEST_MARKER";
let received = "";

const timeout = setTimeout(() => {
  console.error(`timed out waiting for the PTY session to end (received so far: ${JSON.stringify(received)})`);
  process.exit(1);
}, 8000);

ws.addEventListener("open", () => {
  if (disconnectOnly) {
    // No "exit" sent -- the remote shell is still running when this
    // closes, exercising the host-disconnects-first hangup path.
    ws.close();
    return;
  }
  ws.send(Buffer.from(`echo ${MARKER}\nexit\n`));
});

ws.addEventListener("message", (event) => {
  const chunk = event.data instanceof ArrayBuffer ? Buffer.from(event.data).toString("utf8") : String(event.data);
  received += chunk;
});

ws.addEventListener("close", () => {
  clearTimeout(timeout);
  if (disconnectOnly) {
    process.exit(0);
  }
  if (received.includes(MARKER)) {
    process.exit(0);
  }
  console.error(`session closed without seeing the marker in its output: ${JSON.stringify(received)}`);
  process.exit(1);
});

ws.addEventListener("error", (event) => {
  clearTimeout(timeout);
  console.error(`WebSocket error: ${event.message ?? event}`);
  process.exit(1);
});

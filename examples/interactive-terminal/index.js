import { Sandbox } from "sandkiln";

// A live, bidirectional shell session inside a sandbox -- distinct from
// `sandbox.runCommand()`, which is request-in/response-out. `pty()`
// returns a plain WebSocket; everything else here is just wiring that up
// to this process's own terminal, the same shape `kiln sandbox pty`
// itself uses.

async function main() {
  console.log("Creating sandbox...");
  const sandbox = await Sandbox.create({ tags: { example: "interactive-terminal" } });
  console.log(`Sandbox ${sandbox.id} ready. Opening a shell -- type 'exit' or press Ctrl+D to end the session.\n`);

  const cols = process.stdout.columns || 80;
  const rows = process.stdout.rows || 24;
  const ws = sandbox.pty({ cols, rows });
  ws.binaryType = "arraybuffer";

  const wasRaw = process.stdin.isTTY ? process.stdin.isRaw : undefined;
  const restoreTerminal = () => {
    if (process.stdin.isTTY && wasRaw !== undefined) {
      process.stdin.setRawMode(wasRaw);
    }
    process.stdin.pause();
    process.stdin.removeAllListeners("data");
  };

  try {
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", () => {
        if (process.stdin.isTTY) {
          process.stdin.setRawMode(true);
        }
        process.stdin.resume();
        // Every keystroke, including Ctrl+C, goes straight to the remote
        // shell -- raw mode means Node never intercepts it as SIGINT, the
        // same way a real terminal or SSH client behaves.
        process.stdin.on("data", (chunk) => ws.send(chunk));
      });

      ws.addEventListener("message", (event) => {
        const buf = event.data instanceof ArrayBuffer ? Buffer.from(event.data) : Buffer.from(String(event.data));
        process.stdout.write(buf);
      });

      ws.addEventListener("close", () => {
        restoreTerminal();
        resolve();
      });
      ws.addEventListener("error", (event) => {
        restoreTerminal();
        reject(event.error ?? new Error("PTY WebSocket error"));
      });
    });
  } finally {
    await sandbox.stop();
    console.log(`\nSandbox ${sandbox.id} stopped.`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});

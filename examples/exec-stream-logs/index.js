import { Sandbox } from "sandkiln";

const STEPS = 6;
// A stand-in for something long-running worth watching (a build, a test
// run, a migration) -- multiple lines over several seconds, so the
// difference between a live tail and a replay is actually observable.
const COMMAND = "sh";
const ARGS = [
  "-c",
  `for i in $(seq 1 ${STEPS}); do echo "[step $i/${STEPS}] building module-$i"; sleep 1; done; echo "build finished"`,
];

function attach(sandbox, sessionId, onChunk) {
  return new Promise((resolve, reject) => {
    const ws = sandbox.attachLogs(sessionId);
    ws.binaryType = "arraybuffer";
    let text = "";
    ws.addEventListener("message", (event) => {
      const chunk = event.data instanceof ArrayBuffer ? Buffer.from(event.data).toString("utf8") : String(event.data);
      text += chunk;
      onChunk?.(chunk);
    });
    // The daemon closes the socket once it has sent the exit notice, so
    // "closed" means "the whole log has arrived", not "connection lost".
    ws.addEventListener("close", () => resolve(text));
    ws.addEventListener("error", (event) => reject(event.error ?? new Error("logs WebSocket error")));
  });
}

function indent(text) {
  return text.trimEnd().split("\n").map((line) => `    | ${line}`).join("\n");
}

async function main() {
  console.log("Creating sandbox...");
  const sandbox = await Sandbox.create({ tags: { example: "exec-stream-logs" } });
  console.log(`Sandbox ${sandbox.id} ready.`);

  try {
    const sessionId = await sandbox.execStream(COMMAND, ARGS);
    console.log(`Started background session ${sessionId} -- execStream() returned immediately; the command is still running.\n`);

    console.log("Attaching while it runs (replay of anything captured so far, then a live tail):");
    const liveStarted = performance.now();
    const live = await attach(sandbox, sessionId, (chunk) => process.stdout.write(indent(chunk) + "\n"));
    console.log(`  ...attached for ${((performance.now() - liveStarted) / 1000).toFixed(1)}s, ${live.length} bytes.\n`);

    const [session] = await sandbox.listExecStreams();
    console.log(`listExecStreams(): ${session.command} ${session.args.join(" ")} -- exit code ${session.exitCode}, started ${session.startedAt.toISOString()}.\n`);

    console.log("Reattaching to the same, now-finished session:");
    const replayStarted = performance.now();
    const replay = await attach(sandbox, sessionId);
    const replayMs = performance.now() - replayStarted;
    console.log(indent(replay));
    console.log(`  ...${replay.length} bytes in ${replayMs.toFixed(0)}ms.\n`);

    if (replay === live) {
      console.log(
        `The reattach got the identical ${replay.length} bytes in ${replayMs.toFixed(0)}ms rather than the ${((performance.now() - liveStarted) / 1000).toFixed(0)}s the live attach took, ` +
          "because the daemon buffers the session's output independently of any one connection -- nothing was attached when most of it was produced.",
      );
    } else {
      console.log("The reattach differed from the live attach -- see the two blocks above.");
    }
  } finally {
    await sandbox.stop({ keep: false });
    console.log(`\nSandbox ${sandbox.id} stopped.`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});

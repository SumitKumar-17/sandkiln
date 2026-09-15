import { Sandbox } from "sandkiln";

// Sandbox.getOrCreate({ name }) resolves a name to a sandbox in one call:
// a live sandbox with this name is returned as-is, a stopped (snapshotted)
// one is resumed, and otherwise a fresh one is created and given the
// name. Sandbox.stop() defaults to keeping state (snapshot + retire, not
// destroy) precisely so a name can be picked back up later -- unlike most
// sandbox platforms, where "stop" usually means "gone."
//
// That combination is the actual feature this example demonstrates: a
// long-running agent (a coding assistant with a persistent workspace, a
// per-user scratch environment, a stateful background job) can stop
// paying for compute between turns without losing anything, and doesn't
// need to track a sandbox id anywhere -- the name is enough.
//
// This runs two simulated "process invocations" back to back, in the
// same script, stopping and reconnecting by name in between, so the
// persistence claim is proven in one run rather than asking you to
// trust two separate `node index.js` invocations. Running this file
// twice for real (`node index.js`, then `node index.js` again) shows the
// identical behavior across genuinely separate processes.

const NAME = "example-counter-agent";
const COUNTER_PATH = "/tmp/run-count.txt";

async function simulateOneRun(label) {
  console.log(`--- ${label}: Sandbox.getOrCreate({ name: "${NAME}" }) ---`);
  const { sandbox, created } = await Sandbox.getOrCreate({ name: NAME });
  console.log(created ? "  no sandbox with this name existed -- created fresh." : "  found this name -- resumed its prior state.");

  const previous = await sandbox
    .readFile(COUNTER_PATH)
    .then((bytes) => parseInt(new TextDecoder().decode(bytes).trim(), 10))
    .catch(() => 0);
  const current = previous + 1;
  await sandbox.writeFile(COUNTER_PATH, `${current}\n`);
  console.log(`  run count: ${previous} -> ${current} (written to ${COUNTER_PATH} inside the sandbox)`);

  // No { keep: false } here -- the default keeps state. The sandbox id
  // stops existing, same as any stop, but its state becomes a snapshot
  // still reachable under this same name.
  const { kept, snapshotId } = await sandbox.stop();
  console.log(`  stopped. kept=${kept}, snapshotId=${snapshotId}\n`);
}

async function main() {
  await simulateOneRun("First invocation");
  await simulateOneRun("Second invocation (a fresh process, in reality)");

  console.log("Cleaning up: resolving the name one last time and destroying it for real...");
  const { sandbox } = await Sandbox.getOrCreate({ name: NAME });
  await sandbox.stop({ keep: false });
  console.log(`Done. If the count above went 0 -> 1 -> 2, persistence-by-default worked as claimed.`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});

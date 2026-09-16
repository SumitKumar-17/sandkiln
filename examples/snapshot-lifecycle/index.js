import { Sandbox } from "sandkiln";

// The core idea: pay a setup cost once, then reuse the resulting state as
// many times as you want without repeating it.
//
// Sandbox.snapshot() freezes a sandbox's full state (memory + disk) to
// disk and stops it -- the sandbox id itself is retired.
// Sandbox.fork(snapshotId) boots a new, independent sandbox from that
// frozen state WITHOUT consuming it, so the same snapshot can be forked
// again later. Sandbox.resume(snapshotId) also boots from it, but DOES
// consume it -- a resumed snapshot can't be resumed or forked again.
//
// This example does the expensive setup once, forks two independent
// branches from the identical frozen state and proves neither branch's
// writes leak into the other, then resumes the snapshot to show that a
// resume really does retire it for good.

async function main() {
  console.log("Creating a sandbox and doing one-time setup...");
  const sandbox = await Sandbox.create({ tags: { example: "snapshot-lifecycle" } });

  // Stand-in for something that's actually expensive to redo: installing
  // dependencies, downloading a dataset, warming a cache. A real setup
  // step lives here instead.
  await sandbox.writeFile("/tmp/base-state.txt", "expensive setup ran once\n");
  console.log("Setup done. Freezing state with snapshot()...");

  const snapshotId = await sandbox.snapshot();
  console.log(`Snapshot ${snapshotId} taken -- the original sandbox id is now retired.\n`);

  console.log("Forking branch A from the frozen snapshot...");
  const branchA = await Sandbox.fork(snapshotId);
  const baseFromA = new TextDecoder().decode(await branchA.readFile("/tmp/base-state.txt"));
  console.log(`  branch A sees the base state: ${baseFromA.trim()}`);
  await branchA.writeFile("/tmp/branch-marker.txt", "written by branch A\n");
  await branchA.stop({ keep: false });
  console.log("  branch A stopped.\n");

  console.log("Forking branch B from the SAME snapshot...");
  // Only one live fork of a given snapshot can exist at a time, so branch
  // A had to be stopped above before this call -- a second fork() while
  // an earlier one is still running would reject with a 409.
  const branchB = await Sandbox.fork(snapshotId);
  const baseFromB = new TextDecoder().decode(await branchB.readFile("/tmp/base-state.txt"));
  console.log(`  branch B sees the same base state: ${baseFromB.trim()}`);
  const branchAMarkerLeaked = await branchB
    .readFile("/tmp/branch-marker.txt")
    .then(() => true)
    .catch(() => false);
  console.log(
    branchAMarkerLeaked
      ? "  unexpected: branch A's write is visible in branch B"
      : "  confirmed: branch A's write did not leak into branch B -- each fork is independent",
  );
  await branchB.stop({ keep: false });
  console.log("  branch B stopped.\n");

  console.log("Resuming the snapshot (this consumes it)...");
  const resumed = await Sandbox.resume(snapshotId);
  const baseFromResume = new TextDecoder().decode(await resumed.readFile("/tmp/base-state.txt"));
  console.log(`  resumed sandbox sees the base state too: ${baseFromResume.trim()}`);

  const stillForkable = await Sandbox.fork(snapshotId)
    .then(() => true)
    .catch(() => false);
  console.log(
    stillForkable
      ? "  unexpected: the snapshot is still usable after a resume"
      : "  confirmed: the snapshot is gone -- resume() retires it, unlike fork()",
  );

  await resumed.stop({ keep: false });
  console.log("\nDone.");
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});

import { Pool, Sandbox } from "sandkiln";

// Pre-warmed pools are entirely transparent: a plain Sandbox.create()
// automatically resumes a warm snapshot instead of cold-booting whenever
// one matches and is ready -- there's no separate "claim" call.
//
// This example runs several claims in a row rather than just one,
// because a single measurement can be misleading here: resuming a
// snapshot has a real Firecracker/KVM failure mode (a rare guest-kernel
// panic on restore) that a well-behaved pool claim detects with a
// post-resume health check and transparently falls back from -- safely,
// but at the cost of a few extra seconds instead of the fast path. See
// ROADMAP.md's "Persistence and snapshotting" section for the full
// finding. Reporting only a lucky fast run would overstate what to
// expect; reporting several runs shows both outcomes honestly.

const POOL_ID = `pool-warm-start-example-${Date.now()}`;
const ATTEMPTS = 5;
// A clean claim is fast (tens to low hundreds of ms); a health-check
// fallback pays a multi-second penalty recovering from a bad resume --
// there is no middle ground, so a simple threshold reliably tells them
// apart without needing access to the daemon's own logs.
const FALLBACK_THRESHOLD_MS = 2000;

async function waitForWarm(poolId, want, { timeoutMs = 30_000, intervalMs = 2000 } = {}) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const pools = await Pool.list();
    const pool = pools.find((p) => p.id === poolId);
    if (pool && pool.warmReady >= want) return pool;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
  throw new Error(`pool ${poolId} never reached warmReady >= ${want} within ${timeoutMs}ms`);
}

async function main() {
  console.log(`Configuring pool "${POOL_ID}" with warmCount: 1...`);
  await Pool.create(POOL_ID, { warmCount: 1 });

  try {
    const results = [];
    for (let i = 1; i <= ATTEMPTS; i++) {
      process.stdout.write(`Attempt ${i}/${ATTEMPTS}: waiting for a warm snapshot... `);
      await waitForWarm(POOL_ID, 1);

      const started = performance.now();
      // Matches the pool's image/resources (both omitted here, so both
      // resolve to the daemon's own defaults on each side) and has no
      // drives/rateLimit -- claims the warm snapshot automatically.
      const sandbox = await Sandbox.create({ tags: { example: "pool-warm-start" } });
      const elapsedMs = performance.now() - started;

      const clean = elapsedMs < FALLBACK_THRESHOLD_MS;
      console.log(clean ? `clean claim, ${elapsedMs.toFixed(0)}ms` : `health check failed, fell back to a cold create (${elapsedMs.toFixed(0)}ms)`);
      results.push({ clean, elapsedMs });

      await sandbox.stop({ keep: false });
    }

    const cleanRuns = results.filter((r) => r.clean);
    const fallbackRuns = results.filter((r) => !r.clean);
    console.log(`\n${cleanRuns.length}/${ATTEMPTS} clean, ${fallbackRuns.length}/${ATTEMPTS} fell back to a cold create.`);
    if (cleanRuns.length > 0) {
      const avgClean = cleanRuns.reduce((sum, r) => sum + r.elapsedMs, 0) / cleanRuns.length;
      console.log(`Average clean-claim latency: ${avgClean.toFixed(0)}ms.`);
    }
    if (fallbackRuns.length > 0) {
      console.log(
        "Every fallback still succeeded -- the pool's post-resume health check caught a bad resume and " +
          "recovered instead of handing back a broken sandbox. This is expected behavior, not a bug in this example.",
      );
    }
  } finally {
    await Pool.delete(POOL_ID);
    console.log(`\nPool "${POOL_ID}" deleted.`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});

import { resolveClient } from "./client.js";
import { request } from "./http.js";
import type { DaemonStatus, DaemonStatusBody, PoolOptions } from "./types.js";

/**
 * Host health and capacity in one call: version, uptime, object counts,
 * tap-device and disk headroom, rootfs clone methods, and plain-language
 * `warnings` an operator should act on. Backs `kiln status`.
 */
export async function getStatus(options: PoolOptions = {}): Promise<DaemonStatus> {
  const client = resolveClient(options);
  const body = await request<DaemonStatusBody>({ ...client, method: "GET", path: "/status" });
  return {
    version: body.version,
    uptimeSecs: body.uptime_secs,
    authEnabled: body.auth_enabled,
    jailerEnabled: body.jailer_enabled,
    counts: {
      sandboxes: body.counts.sandboxes,
      heldSnapshots: body.counts.held_snapshots,
      retiredCheckpoints: body.counts.retired_checkpoints,
      pools: body.counts.pools,
      poolWarmReady: body.counts.pool_warm_ready,
    },
    tapPool: { size: body.tap_pool.size, free: body.tap_pool.free, inUse: body.tap_pool.in_use },
    ipPool: body.ip_pool && { size: body.ip_pool.size, free: body.ip_pool.free, heldBySnapshots: body.ip_pool.held_by_snapshots },
    sandboxesCreatedTotal: body.sandboxes_created_total,
    rootfsClones: body.rootfs_clones,
    disk: body.disk.map((d) => ({ path: d.path, totalBytes: d.total_bytes, freeBytes: d.free_bytes })),
    warnings: body.warnings,
  };
}

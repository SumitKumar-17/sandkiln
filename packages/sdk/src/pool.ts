import { resolveClient } from "./client.js";
import { request } from "./http.js";
import type { CreatePoolRequestBody, ListPoolsResponseBody, PoolInfo, PoolOptions, PoolSummaryBody } from "./types.js";

/**
 * Pre-warmed pools: keep a small number of ready-to-resume snapshots
 * around for a given image/resource config, so a matching
 * `Sandbox.create()` can resume one instead of paying full cold-create
 * cost. Like `Image`/`Drive`, this is a namespace of static operations
 * rather than a stateful handle class — a pool has no instance behavior
 * besides delete.
 *
 * Claiming from a pool is entirely transparent: `Sandbox.create()` itself
 * matches a plain create (no `drives`, no `rateLimit`) against every
 * configured pool by `imageId`/`vcpuCount`/`memSizeMib` and resumes a
 * warm snapshot automatically when one is ready — there's no separate
 * "create from pool" call. `maxCount` caps the total live instances
 * (warm + claimed) a pool's profile may have at once; a claim arriving
 * at that ceiling with nothing warm queues daemon-side (up to 30s)
 * before the underlying `Sandbox.create()` call rejects with a `503`.
 * See `ROADMAP.md`'s "Persistence and snapshotting" section, "Pre-warmed
 * snapshot pool", for the full design.
 */
export class Pool {
  /**
   * Configures a new pool under `id` — a stable name a caller uses to
   * refer back to this configuration (not exposed to the guest or any
   * sandbox created from it). Rejected (`409`) if `id` is already taken;
   * delete it first to reconfigure. `warmCount` is how many resumable
   * snapshots to keep ready at once — replenishment happens in the
   * background and isn't instant, see `PoolInfo.warmReady`. `maxCount`
   * is omitted for an unbounded pool (the original, still-default
   * behavior) — throws if given as `0`, since that's never a meaningful
   * ceiling.
   */
  static async create(id: string, options: CreatePoolOptions = {}): Promise<PoolInfo> {
    const client = resolveClient(options);
    const requestBody: CreatePoolRequestBody = {
      id,
      image_id: options.imageId,
      vcpu_count: options.vcpuCount,
      mem_size_mib: options.memSizeMib,
      warm_count: options.warmCount ?? 0,
      max_count: options.maxCount,
    };
    const body = await request<PoolSummaryBody>({
      ...client,
      method: "POST",
      path: "/pools",
      body: requestBody,
    });
    return toPoolInfo(body);
  }

  static async list(options: PoolOptions = {}): Promise<PoolInfo[]> {
    const client = resolveClient(options);
    const body = await request<ListPoolsResponseBody>({
      ...client,
      method: "GET",
      path: "/pools",
    });
    return body.pools.map(toPoolInfo);
  }

  /**
   * Removes a pool's configuration and destroys whatever it currently
   * has warm. A sandbox already claimed from this pool is unaffected —
   * only the pool's own not-yet-claimed warm snapshots are cleaned up.
   */
  static async delete(id: string, options: PoolOptions = {}): Promise<void> {
    const client = resolveClient(options);
    await request<void>({
      ...client,
      method: "DELETE",
      path: `/pools/${encodeURIComponent(id)}`,
    });
  }
}

export interface CreatePoolOptions extends PoolOptions {
  /** Boots warm instances from this registered image (see `Image.register`)
   * instead of the daemon's configured default rootfs. A `Sandbox.create()`
   * only ever matches a pool whose `imageId` is exactly the same as its
   * own (both omitted counts as a match). */
  imageId?: string;
  vcpuCount?: number;
  memSizeMib?: number;
  /** How many resumable snapshots to keep ready at once. Defaults to `0`
   * (configures the pool's identity/matching key without ever keeping
   * anything warm) — a strange thing to actually want, but not an error. */
  warmCount?: number;
  /** Maximum live instances (warm + claimed, combined) this pool's
   * profile may ever have at once. Omitted means unbounded — a claim
   * past what's warm always just cold-creates, with nothing capping how
   * many can be live simultaneously. */
  maxCount?: number;
}

function toPoolInfo(body: PoolSummaryBody): PoolInfo {
  return {
    id: body.id,
    imageId: body.image_id,
    vcpuCount: body.vcpu_count,
    memSizeMib: body.mem_size_mib,
    warmCount: body.warm_count,
    maxCount: body.max_count,
    warmReady: body.warm_ready,
    claimed: body.claimed,
  };
}

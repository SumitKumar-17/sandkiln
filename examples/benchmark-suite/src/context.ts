/** What every benchmark receives: connection details, options, a metric
 * sink, and cleanup of everything it created. */

import { Sandbox } from "sandkiln";
import type { Metric, RunOptions, Unit } from "./types.ts";

export interface Connection {
  baseUrl: string;
  authToken?: string;
}

export const RUN_TAG_KEY = "benchmark_run";

export class BenchContext {
  readonly metrics: Record<string, Metric> = {};

  readonly connection: Connection;
  readonly options: RunOptions;
  /** Tag value stamped on every sandbox so cleanup can find exactly ours. */
  readonly runTag: string;

  constructor(connection: Connection, options: RunOptions, runTag: string) {
    this.connection = connection;
    this.options = options;
    this.runTag = runTag;
  }

  record(name: string, value: number, unit: Unit = "ms", higherIsBetter = false): void {
    const metric = (this.metrics[name] ??= { unit, higherIsBetter, samples: [] });
    metric.samples.push(value);
  }

  create(): Promise<Sandbox> {
    return Sandbox.create({
      ...this.connection,
      tags: { [RUN_TAG_KEY]: this.runTag },
      ...(this.options.imageId === null ? {} : { imageId: this.options.imageId }),
    });
  }

  /** Deletes every sandbox and snapshot carrying this run's tag. Safe to call
   * repeatedly; never throws so it can run in `finally` blocks. */
  async cleanup(): Promise<void> {
    const tags = { [RUN_TAG_KEY]: this.runTag };
    try {
      const live = await Sandbox.list({ ...this.connection, tags });
      await Promise.allSettled(live.map((s) => Sandbox.attach(s.id, this.connection).stop({ keep: false })));
      const snapshots = await Sandbox.listSnapshots(this.connection);
      await Promise.allSettled(
        snapshots.filter((s) => s.tags[RUN_TAG_KEY] === this.runTag).map((s) => this.deletePath(`/snapshots/${encodeURIComponent(s.id)}`)),
      );
      // `resume` retires the checkpoint into history by default (a full memory
      // dump plus a rootfs copy per iteration), so sweep that list too.
      const history = await this.getJson<{ checkpoints: { id: string; tags: Record<string, string> }[] }>("/snapshots/history");
      await Promise.allSettled(
        history.checkpoints.filter((c) => c.tags[RUN_TAG_KEY] === this.runTag).map((c) => this.deletePath(`/snapshots/history/${encodeURIComponent(c.id)}`)),
      );
    } catch (error) {
      console.error(`cleanup failed, check for leftovers tagged ${RUN_TAG_KEY}=${this.runTag}:`, error);
    }
  }

  private headers(): Record<string, string> {
    const { authToken } = this.connection;
    return authToken === undefined ? {} : { authorization: `Bearer ${authToken}` };
  }

  private async getJson<T>(path: string): Promise<T> {
    const res = await fetch(`${this.connection.baseUrl}${path}`, { headers: this.headers() });
    if (!res.ok) throw new Error(`GET ${path}: ${res.status}`);
    return (await res.json()) as T;
  }

  private async deletePath(path: string): Promise<void> {
    const res = await fetch(`${this.connection.baseUrl}${path}`, { method: "DELETE", headers: this.headers() });
    if (!res.ok && res.status !== 404) throw new Error(`DELETE ${path}: ${res.status}`);
  }
}

export interface Benchmark {
  name: string;
  /** One line for `--list` and the README. */
  description: string;
  run(ctx: BenchContext): Promise<void>;
}

/** Wall time of an async step, in milliseconds. */
export async function timed<T>(fn: () => Promise<T>): Promise<{ value: T; ms: number }> {
  const start = performance.now();
  const value = await fn();
  return { value, ms: performance.now() - start };
}

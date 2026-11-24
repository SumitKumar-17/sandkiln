/** Ownership of the sandboxes this server instance created.
 *
 * The MCP client is an AI agent that may crash, loop, or simply forget to
 * clean up, so the server (not the model) is responsible for stopping what
 * it started when the connection ends. */

import { ToolFailure } from "./errors.js";

export interface Stopper {
  (id: string, keep: boolean): Promise<unknown>;
}

export class SandboxSession {
  private readonly live = new Set<string>();

  constructor(
    private readonly maxSandboxes: number,
    private readonly stop: Stopper,
  ) {}

  /** Throws before the daemon is asked to boot anything over the cap. */
  assertCanCreate(): void {
    if (this.live.size >= this.maxSandboxes) {
      throw new ToolFailure(
        "limit_reached",
        `this session already has ${this.live.size} live sandboxes (limit ${this.maxSandboxes})`,
        "Stop a sandbox with sandbox_stop first, or raise SANDKILN_MCP_MAX_SANDBOXES.",
      );
    }
  }

  track(id: string): void {
    this.live.add(id);
  }

  /** Called when a sandbox stops or is retired by a snapshot. */
  forget(id: string): void {
    this.live.delete(id);
  }

  owns(id: string): boolean {
    return this.live.has(id);
  }

  ids(): string[] {
    return [...this.live];
  }

  /** Stops everything still tracked. One failure never blocks the rest;
   * returns the ids that could not be stopped. */
  async cleanup(keep: boolean): Promise<string[]> {
    const failed: string[] = [];
    await Promise.all(
      this.ids().map(async (id) => {
        try {
          await this.stop(id, keep);
          this.live.delete(id);
        } catch {
          failed.push(id);
        }
      }),
    );
    return failed;
  }
}

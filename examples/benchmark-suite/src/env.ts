/** Collects the environment block: daemon facts from `/status`, the guest
 * kernel from inside a sandbox, and host kernel/CPU when the daemon is local. */

import { readFileSync } from "node:fs";
import { release } from "node:os";
import type { BenchContext } from "./context.ts";
import { getStatus } from "./status.ts";
import type { Environment } from "./types.ts";

export function isLoopback(baseUrl: string): boolean {
  const host = new URL(baseUrl).hostname;
  return host === "localhost" || host === "::1" || host === "[::1]" || /^127\./.test(host);
}

export function parseCpuModel(cpuinfo: string): string | null {
  const match = /^model name\s*:\s*(.+)$/m.exec(cpuinfo);
  const model = match?.[1];
  if (model === undefined) return null;
  const threads = (cpuinfo.match(/^processor\s*:/gm) ?? []).length;
  return `${model.trim()} (${threads} threads)`;
}

export async function collectEnvironment(ctx: BenchContext, hardwareLabel: string | null): Promise<Environment> {
  const status = await getStatus(ctx.connection);
  const local = isLoopback(ctx.connection.baseUrl);
  let hostCpu: string | null = null;
  if (local) {
    try {
      hostCpu = parseCpuModel(readFileSync("/proc/cpuinfo", "utf8"));
    } catch {
      // Not Linux (a macOS client talking to a loopback tunnel): leave unknown.
    }
  }
  return {
    date: new Date().toISOString(),
    daemonUrl: ctx.connection.baseUrl,
    daemonVersion: status.version,
    daemonUptimeSecs: status.uptime_secs,
    hardwareLabel,
    hostKernel: local && process.platform === "linux" ? release() : null,
    hostCpu,
    guestKernel: await guestKernel(ctx),
    rootfsClones: status.rootfs_clones,
    liveSandboxesAtStart: status.counts.sandboxes,
    node: process.version,
  };
}

async function guestKernel(ctx: BenchContext): Promise<string | null> {
  const sandbox = await ctx.create();
  try {
    const result = await sandbox.runCommand("uname", ["-r"]);
    return result.exitCode === 0 ? result.stdout.trim() : null;
  } finally {
    await sandbox.stop({ keep: false });
  }
}

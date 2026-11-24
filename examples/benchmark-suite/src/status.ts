/** `GET /status` called directly: only a few fields are needed, and it keeps
 * this module independent of the SDK's status types. */

import type { Connection } from "./context.ts";

export interface DaemonStatus {
  version: string;
  uptime_secs: number;
  counts: { sandboxes: number; held_snapshots: number };
  rootfs_clones: Record<string, number>;
}

export async function getStatus(connection: Connection): Promise<DaemonStatus> {
  const headers: Record<string, string> = {};
  if (connection.authToken !== undefined) headers.authorization = `Bearer ${connection.authToken}`;
  const res = await fetch(`${connection.baseUrl}/status`, { headers });
  if (!res.ok) throw new Error(`GET ${connection.baseUrl}/status failed: ${res.status} ${res.statusText}`);
  return (await res.json()) as DaemonStatus;
}

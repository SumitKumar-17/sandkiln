export interface SandboxOptions {
  baseUrl?: string;
  authToken?: string;
}

export interface CreateSandboxOptions extends SandboxOptions {
  /** Caller-given identity, unique among live sandboxes and held
   * snapshots at the moment it's claimed — the daemon rejects a taken
   * name with a 409. Optional; omit for an anonymous sandbox, same as
   * before this existed. See `Sandbox.byName`/`Sandbox.getOrCreate` for
   * finding a named sandbox again later. */
  name?: string;
  tags?: Record<string, string>;
  /** Overrides the daemon's configured default vCPU count for this one
   * sandbox. Rejected by the daemon if it's `0` or exceeds the daemon's
   * configured ceiling (`SANDKILN_MAX_VCPU_COUNT`). */
  vcpuCount?: number;
  /** Overrides the daemon's configured default memory size (MiB) for
   * this one sandbox. Same ceiling semantics as `vcpuCount`, checked
   * against `SANDKILN_MAX_MEM_SIZE_MIB`. */
  memSizeMib?: number;
}

/** `tags`/`vcpuCount`/`memSizeMib` are used only when
 * `Sandbox.getOrCreate` actually creates a fresh sandbox — resuming an
 * existing snapshot under this name ignores them, using what was
 * recorded on it when it was taken, same as `Sandbox.resume`. */
export interface GetOrCreateSandboxOptions extends SandboxOptions {
  name: string;
  tags?: Record<string, string>;
  vcpuCount?: number;
  memSizeMib?: number;
}

export interface ListSandboxesOptions extends SandboxOptions {
  /** Only sandboxes matching every given tag are returned. */
  tags?: Record<string, string>;
}

export interface SandboxInfo {
  id: string;
  createdAt: Date;
  tags: Record<string, string>;
  name?: string;
}

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export interface CreateSandboxRequestBody {
  name?: string;
  tags?: Record<string, string>;
  vcpu_count?: number;
  mem_size_mib?: number;
}

export interface CreateSandboxResponseBody {
  id: string;
}

export interface SandboxSummaryBody {
  id: string;
  created_at_unix: number;
  tags: Record<string, string>;
  name?: string | null;
}

export interface ListSandboxesResponseBody {
  sandboxes: SandboxSummaryBody[];
}

export interface ExecRequestBody {
  command: string;
  args: string[];
}

export interface ExecResponseBody {
  stdout: string;
  stderr: string;
  exit_code: number;
}

export interface ReadFileRequestBody {
  path: string;
}

export interface ReadFileResponseBody {
  content_base64: string;
}

export interface WriteFileRequestBody {
  path: string;
  content_base64: string;
}

export interface PreviewUrlOptions {
  /** Path within the guest's server to preview, e.g. `/api/health`.
   * Defaults to `/`. A value with no leading slash gets one added. */
  path?: string;
}

export interface ApiErrorBody {
  error: string;
}

export interface SnapshotSandboxResponseBody {
  snapshot_id: string;
}

export interface ResumeSnapshotResponseBody {
  id: string;
}

export interface ForkSnapshotResponseBody {
  id: string;
}

/** Present on the response body only when the stop actually reported
 * something (`keep=true`, the default) — `DELETE /sandboxes/:id
 * ?keep=false` returns 204 with no body, decoded as `undefined` by
 * `request()`. See `Sandbox.stop`. */
export interface StopSandboxResponseBody {
  kept: boolean;
  snapshot_id: string | null;
}

export interface SandboxByNameResponseBody {
  id: string;
}

export interface GetOrCreateSandboxRequestBody {
  name: string;
  tags?: Record<string, string>;
  vcpu_count?: number;
  mem_size_mib?: number;
}

export interface GetOrCreateSandboxResponseBody {
  id: string;
  created: boolean;
}

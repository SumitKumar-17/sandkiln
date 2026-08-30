export interface SandboxOptions {
  baseUrl?: string;
  authToken?: string;
}

export interface CreateSandboxOptions extends SandboxOptions {
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

export interface ListSandboxesOptions extends SandboxOptions {
  /** Only sandboxes matching every given tag are returned. */
  tags?: Record<string, string>;
}

export interface SandboxInfo {
  id: string;
  createdAt: Date;
  tags: Record<string, string>;
}

export interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

export interface CreateSandboxRequestBody {
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

export interface ApiErrorBody {
  error: string;
}

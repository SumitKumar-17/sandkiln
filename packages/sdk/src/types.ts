export interface SandboxOptions {
  baseUrl?: string;
  authToken?: string;
}

export interface CreateSandboxOptions extends SandboxOptions {
  tags?: Record<string, string>;
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

export interface PreviewUrlOptions {
  /** Path within the guest's server to preview, e.g. `/api/health`.
   * Defaults to `/`. A value with no leading slash gets one added. */
  path?: string;
}

export interface ApiErrorBody {
  error: string;
}

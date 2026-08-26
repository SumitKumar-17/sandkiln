import { resolveBaseUrl } from "./config.js";
import { request } from "./http.js";
import type {
  CreateSandboxResponseBody,
  ExecRequestBody,
  ExecResponseBody,
  ExecResult,
  ListSandboxesResponseBody,
  SandboxInfo,
  SandboxOptions,
} from "./types.js";

export class Sandbox {
  readonly id: string;
  private readonly baseUrl: string;

  private constructor(id: string, baseUrl: string) {
    this.id = id;
    this.baseUrl = baseUrl;
  }

  static async create(options: SandboxOptions = {}): Promise<Sandbox> {
    const baseUrl = resolveBaseUrl(options.baseUrl);
    const body = await request<CreateSandboxResponseBody>({
      baseUrl,
      method: "POST",
      path: "/sandboxes",
    });
    return new Sandbox(body.id, baseUrl);
  }

  static async list(options: SandboxOptions = {}): Promise<SandboxInfo[]> {
    const baseUrl = resolveBaseUrl(options.baseUrl);
    const body = await request<ListSandboxesResponseBody>({
      baseUrl,
      method: "GET",
      path: "/sandboxes",
    });
    return body.sandboxes.map((summary) => ({
      id: summary.id,
      createdAt: new Date(summary.created_at_unix * 1000),
    }));
  }

  async runCommand(command: string, args: string[] = []): Promise<ExecResult> {
    const requestBody: ExecRequestBody = { command, args };
    const body = await request<ExecResponseBody>({
      baseUrl: this.baseUrl,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/exec`,
      body: requestBody,
    });
    return { stdout: body.stdout, stderr: body.stderr, exitCode: body.exit_code };
  }

  async stop(): Promise<void> {
    await request<void>({
      baseUrl: this.baseUrl,
      method: "DELETE",
      path: `/sandboxes/${encodeURIComponent(this.id)}`,
    });
  }
}

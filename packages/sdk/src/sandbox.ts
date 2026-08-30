import { decodeBase64, encodeBase64 } from "./base64.js";
import { resolveAuthToken, resolveBaseUrl } from "./config.js";
import { request } from "./http.js";
import type {
  CreateSandboxOptions,
  CreateSandboxRequestBody,
  CreateSandboxResponseBody,
  ExecRequestBody,
  ExecResponseBody,
  ExecResult,
  ListSandboxesOptions,
  ListSandboxesResponseBody,
  PreviewUrlOptions,
  ReadFileRequestBody,
  ReadFileResponseBody,
  SandboxInfo,
  SandboxOptions,
  WriteFileRequestBody,
} from "./types.js";

interface ClientContext {
  baseUrl: string;
  authToken?: string;
}

export class Sandbox {
  readonly id: string;
  private readonly client: ClientContext;

  private constructor(id: string, client: ClientContext) {
    this.id = id;
    this.client = client;
  }

  static async create(options: CreateSandboxOptions = {}): Promise<Sandbox> {
    const client = resolveClient(options);
    const requestBody: CreateSandboxRequestBody | undefined =
      options.tags !== undefined ? { tags: options.tags } : undefined;
    const body = await request<CreateSandboxResponseBody>({
      ...client,
      method: "POST",
      path: "/sandboxes",
      body: requestBody,
    });
    return new Sandbox(body.id, client);
  }

  /**
   * Wraps an already-existing sandbox id without a network round-trip —
   * for callers (like the CLI) that only have an id from a previous
   * process and need a handle to call instance methods on. Doesn't
   * verify the sandbox actually exists; the first call against it will
   * fail with a 404 if it doesn't.
   */
  static attach(id: string, options: SandboxOptions = {}): Sandbox {
    return new Sandbox(id, resolveClient(options));
  }

  static async list(options: ListSandboxesOptions = {}): Promise<SandboxInfo[]> {
    const client = resolveClient(options);
    const query = new URLSearchParams();
    for (const [key, value] of Object.entries(options.tags ?? {})) {
      query.set(`tag.${key}`, value);
    }
    const suffix = query.size > 0 ? `?${query.toString()}` : "";

    const body = await request<ListSandboxesResponseBody>({
      ...client,
      method: "GET",
      path: `/sandboxes${suffix}`,
    });
    return body.sandboxes.map((summary) => ({
      id: summary.id,
      createdAt: new Date(summary.created_at_unix * 1000),
      tags: summary.tags,
    }));
  }

  async runCommand(command: string, args: string[] = []): Promise<ExecResult> {
    const requestBody: ExecRequestBody = { command, args };
    const body = await request<ExecResponseBody>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/exec`,
      body: requestBody,
    });
    return { stdout: body.stdout, stderr: body.stderr, exitCode: body.exit_code };
  }

  async readFile(path: string): Promise<Uint8Array> {
    const requestBody: ReadFileRequestBody = { path };
    const body = await request<ReadFileResponseBody>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/read-file`,
      body: requestBody,
    });
    return decodeBase64(body.content_base64);
  }

  async writeFile(path: string, content: string | Uint8Array): Promise<void> {
    const requestBody: WriteFileRequestBody = { path, content_base64: encodeBase64(content) };
    await request<void>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/write-file`,
      body: requestBody,
    });
  }

  async stop(): Promise<void> {
    await request<void>({
      ...this.client,
      method: "DELETE",
      path: `/sandboxes/${encodeURIComponent(this.id)}`,
    });
  }

  /**
   * The URL a browser can open directly to reach a dev server (or any
   * other HTTP server) listening on `port` inside this sandbox, proxied
   * through the daemon's `/sandboxes/:id/preview/:port` route. Pure and
   * network-free, like `attach` — the daemon proxies lazily on each
   * request, so there's nothing to create or await up front.
   *
   * If this sandbox's client has an auth token configured, it's appended
   * as a `?token=` query parameter rather than sent as a header: the
   * caller of this URL is typically a browser tab or an `<iframe src=...>`
   * embed, neither of which can attach an `Authorization` header, and the
   * daemon's preview route accepts the token either way (see
   * `auth::require_preview_token` on the daemon side).
   */
  previewUrl(port: number, options: PreviewUrlOptions = {}): string {
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
      throw new RangeError(`invalid preview port: ${port}`);
    }
    const rawPath = options.path ?? "/";
    const path = rawPath.startsWith("/") ? rawPath : `/${rawPath}`;

    const query = new URLSearchParams();
    if (this.client.authToken !== undefined) {
      query.set("token", this.client.authToken);
    }
    const suffix = query.size > 0 ? `?${query.toString()}` : "";

    return `${this.client.baseUrl}/sandboxes/${encodeURIComponent(this.id)}/preview/${port}${path}${suffix}`;
  }
}

function resolveClient(options: { baseUrl?: string; authToken?: string }): ClientContext {
  return { baseUrl: resolveBaseUrl(options.baseUrl), authToken: resolveAuthToken(options.authToken) };
}

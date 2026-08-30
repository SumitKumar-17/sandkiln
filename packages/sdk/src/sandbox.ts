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
  ForkSnapshotResponseBody,
  ListSandboxesOptions,
  ListSandboxesResponseBody,
  ReadFileRequestBody,
  ReadFileResponseBody,
  ResumeSnapshotResponseBody,
  SandboxInfo,
  SandboxOptions,
  SnapshotSandboxResponseBody,
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
   * Saves this sandbox's full state (memory + disk) to disk and stops it,
   * returning a snapshot id. The sandbox itself stops existing — call
   * `Sandbox.resume` or `Sandbox.fork` on the returned id to boot from it
   * again.
   */
  async snapshot(): Promise<string> {
    const body = await request<SnapshotSandboxResponseBody>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/snapshot`,
    });
    return body.snapshot_id;
  }

  /**
   * Boots a new sandbox from a snapshot, consuming it — the snapshot is
   * gone afterward, and the new sandbox owns its state outright, the same
   * as one from `Sandbox.create`. Use `Sandbox.fork` instead if you want
   * to boot from the same snapshot more than once.
   */
  static async resume(snapshotId: string, options: SandboxOptions = {}): Promise<Sandbox> {
    const client = resolveClient(options);
    const body = await request<ResumeSnapshotResponseBody>({
      ...client,
      method: "POST",
      path: `/snapshots/${encodeURIComponent(snapshotId)}/resume`,
    });
    return new Sandbox(body.id, client);
  }

  /**
   * Boots a new sandbox from a snapshot *without* consuming it, so the
   * same snapshot can be forked or resumed again later — the building
   * block for starting parallel branches off one prepared environment
   * without repeating its setup cost.
   *
   * Only one live sandbox forked from a given snapshot may exist at a
   * time: a fork reopens the exact rootfs file the snapshot recorded
   * (and, if the original sandbox was networked, the exact tap device —
   * its guest IP/MAC were frozen in at that sandbox's original boot), so
   * two live forks at once would mean either two VMs writing the same
   * disk image or two guests colliding on one IP/MAC. A second `fork()`
   * call while an earlier fork is still running rejects with a 409 until
   * that one is stopped.
   */
  static async fork(snapshotId: string, options: SandboxOptions = {}): Promise<Sandbox> {
    const client = resolveClient(options);
    const body = await request<ForkSnapshotResponseBody>({
      ...client,
      method: "POST",
      path: `/snapshots/${encodeURIComponent(snapshotId)}/fork`,
    });
    return new Sandbox(body.id, client);
  }
}

function resolveClient(options: { baseUrl?: string; authToken?: string }): ClientContext {
  return { baseUrl: resolveBaseUrl(options.baseUrl), authToken: resolveAuthToken(options.authToken) };
}

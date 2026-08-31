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
  GetOrCreateSandboxOptions,
  GetOrCreateSandboxRequestBody,
  GetOrCreateSandboxResponseBody,
  ListSandboxesOptions,
  ListSandboxesResponseBody,
  ListSnapshotsOptions,
  ListSnapshotsResponseBody,
  PreviewUrlOptions,
  ReadFileRequestBody,
  ReadFileResponseBody,
  ResumeSnapshotResponseBody,
  SandboxByNameResponseBody,
  SandboxInfo,
  SandboxOptions,
  SnapshotInfo,
  SnapshotSandboxResponseBody,
  StopSandboxResponseBody,
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
    const requestBody = buildCreateSandboxRequestBody(options);
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

  /**
   * A sandbox can drop out of this list on its own, not just from an
   * explicit `stop()`/`snapshot()` call: if the daemon has
   * `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` configured, it pauses and
   * snapshots an idle sandbox automatically — same effect as a manual
   * `snapshot()`. Use `Sandbox.listSnapshots({ sourceSandboxId })` to find
   * out whether a sandbox id that's no longer listed here turned into a
   * snapshot, and its resulting snapshot id.
   */
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
      name: summary.name ?? undefined,
    }));
  }

  /**
   * Resolves a name to a live sandbox and returns a handle to it — a
   * network round-trip, unlike `attach`, since the id isn't known up
   * front. Only resolves a *live* sandbox: if this name currently
   * belongs to a stopped (snapshotted) sandbox instead, the daemon
   * rejects with a 409 rather than silently resuming it — use
   * `Sandbox.getOrCreate` if that's what you want.
   */
  static async byName(name: string, options: SandboxOptions = {}): Promise<Sandbox> {
    const client = resolveClient(options);
    const body = await request<SandboxByNameResponseBody>({
      ...client,
      method: "GET",
      path: `/sandboxes/by-name/${encodeURIComponent(name)}`,
    });
    return new Sandbox(body.id, client);
  }

  /**
   * Resolves a name to a sandbox in one call, creating it if it doesn't
   * exist yet: a live sandbox with this name is returned as-is, a
   * stopped (snapshotted) one is resumed, and otherwise a fresh sandbox
   * is created and given this name. `tags`/`vcpuCount`/`memSizeMib` only
   * apply to the create-fresh case — resuming an existing snapshot uses
   * what was recorded on it when it was taken, same as `Sandbox.resume`.
   *
   * Race-safe on the daemon side: two concurrent calls for the same
   * brand-new name can't both create a sandbox — the second sees the
   * first's result instead.
   */
  static async getOrCreate(options: GetOrCreateSandboxOptions): Promise<{ sandbox: Sandbox; created: boolean }> {
    const client = resolveClient(options);
    const requestBody: GetOrCreateSandboxRequestBody = { name: options.name };
    if (options.tags !== undefined) requestBody.tags = options.tags;
    if (options.vcpuCount !== undefined) requestBody.vcpu_count = options.vcpuCount;
    if (options.memSizeMib !== undefined) requestBody.mem_size_mib = options.memSizeMib;

    const body = await request<GetOrCreateSandboxResponseBody>({
      ...client,
      method: "POST",
      path: "/sandboxes/get-or-create",
      body: requestBody,
    });
    return { sandbox: new Sandbox(body.id, client), created: body.created };
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

  /**
   * Stops this sandbox. By default this *preserves* its state: internally
   * the daemon does what `Sandbox.snapshot()` does (pause, snapshot to
   * disk, stop the VM) and returns the resulting snapshot id — "stop and
   * come back later" is the default, not something you have to manage
   * yourself. Resume it with `Sandbox.resume(snapshotId)`, or find it
   * again by name with `Sandbox.getOrCreate({ name })` if this sandbox
   * had one.
   *
   * Pass `{ keep: false }` for the old "just destroy it" behavior — no
   * snapshot, nothing left behind — for a sandbox you genuinely never
   * want back (e.g. a short-lived CI run).
   */
  async stop(options: { keep?: boolean } = {}): Promise<{ kept: boolean; snapshotId: string | null }> {
    const query = new URLSearchParams();
    if (options.keep !== undefined) {
      query.set("keep", String(options.keep));
    }
    const suffix = query.size > 0 ? `?${query.toString()}` : "";

    // `keep: false` gets a bare 204 back (no body) — `request` decodes
    // that as `undefined`. Normalized here so callers get one consistent
    // shape regardless of which path the daemon took.
    const body = await request<StopSandboxResponseBody | undefined>({
      ...this.client,
      method: "DELETE",
      path: `/sandboxes/${encodeURIComponent(this.id)}${suffix}`,
    });
    if (body === undefined) {
      return { kept: false, snapshotId: null };
    }
    return { kept: body.kept, snapshotId: body.snapshot_id };
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

  /**
   * Saves this sandbox's full state (memory + disk) to disk and stops it,
   * returning a snapshot id. The sandbox itself stops existing — call
   * `Sandbox.resume` or `Sandbox.fork` on the returned id to boot from it
   * again.
   *
   * The daemon can also do this on its own, without a caller ever calling
   * this method, if `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` is configured and
   * this sandbox goes idle past that timeout — see `Sandbox.listSnapshots`.
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

  /**
   * Lists snapshots. `options.sourceSandboxId` narrows this to the (at
   * most one) snapshot taken from that original sandbox id — the way to
   * go from "the sandbox id I had" to "the snapshot it became" after a
   * manual `snapshot()` or the daemon's auto-suspend made it disappear
   * from `Sandbox.list()`. Omitting it lists every snapshot.
   */
  static async listSnapshots(options: ListSnapshotsOptions = {}): Promise<SnapshotInfo[]> {
    const client = resolveClient(options);
    const query = new URLSearchParams();
    if (options.sourceSandboxId !== undefined) {
      query.set("source_sandbox_id", options.sourceSandboxId);
    }
    const suffix = query.size > 0 ? `?${query.toString()}` : "";

    const body = await request<ListSnapshotsResponseBody>({
      ...client,
      method: "GET",
      path: `/snapshots${suffix}`,
    });
    return body.snapshots.map((summary) => ({
      id: summary.id,
      sourceSandboxId: summary.source_sandbox_id,
      createdAt: new Date(summary.created_at_unix * 1000),
      tags: summary.tags,
      forkedInto: summary.forked_into,
      name: summary.name ?? undefined,
    }));
  }
}

function resolveClient(options: { baseUrl?: string; authToken?: string }): ClientContext {
  return { baseUrl: resolveBaseUrl(options.baseUrl), authToken: resolveAuthToken(options.authToken) };
}

/** `undefined` (rather than `{}`) when the caller didn't set anything,
 * matching the daemon's own "empty body means all defaults" handling and
 * this SDK's existing convention for an all-default `POST /sandboxes`. */
function buildCreateSandboxRequestBody(options: CreateSandboxOptions): CreateSandboxRequestBody | undefined {
  const body: CreateSandboxRequestBody = {};
  if (options.name !== undefined) body.name = options.name;
  if (options.tags !== undefined) body.tags = options.tags;
  if (options.vcpuCount !== undefined) body.vcpu_count = options.vcpuCount;
  if (options.memSizeMib !== undefined) body.mem_size_mib = options.memSizeMib;
  return Object.keys(body).length > 0 ? body : undefined;
}

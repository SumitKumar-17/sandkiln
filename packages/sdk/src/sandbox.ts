import { decodeBase64, encodeBase64 } from "./base64.js";
import { resolveClient, type ClientContext } from "./client.js";
import { request } from "./http.js";
import { openTunnel, type TunnelHandle, type TunnelOptions } from "./tunnel.js";
import type {
  ChmodRequestBody,
  ChownRequestBody,
  CopyRequestBody,
  CreateMountRequestBody,
  CreateSandboxOptions,
  CreateSandboxRequestBody,
  CreateSandboxResponseBody,
  DirEntry,
  DriveAttachmentOptions,
  DriveAttachmentRequestBody,
  EgressPolicyOptions,
  EgressPolicyRequestBody,
  ExecRequestBody,
  ExecResponseBody,
  ExecResult,
  ExecStreamSession,
  ForkSnapshotResponseBody,
  GetOrCreateSandboxOptions,
  GetOrCreateSandboxRequestBody,
  GetOrCreateSandboxResponseBody,
  ListDirRequestBody,
  ListDirResponseBody,
  ListExecStreamsResponseBody,
  ListSandboxesOptions,
  ListSandboxesResponseBody,
  ListMountsResponseBody,
  ListSnapshotsOptions,
  ListSnapshotsResponseBody,
  MkdirRequestBody,
  MountInfo,
  MountOptions,
  MountResponseBody,
  PreviewUrlOptions,
  PtyOptions,
  RateLimitOptions,
  RateLimitRequestBody,
  ReadFileRequestBody,
  ReadFileResponseBody,
  ReadlinkRequestBody,
  ReadlinkResponseBody,
  RenameRequestBody,
  ResumeSnapshotResponseBody,
  RunCommandOptions,
  SandboxByNameResponseBody,
  SandboxInfo,
  SandboxOptions,
  SnapshotInfo,
  SnapshotSandboxResponseBody,
  StartExecStreamRequestBody,
  StartExecStreamResponseBody,
  StopSandboxResponseBody,
  SymlinkRequestBody,
  TruncateRequestBody,
  WriteFileRequestBody,
} from "./types.js";

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

  /** Wraps an existing sandbox id with no network round-trip — for a
   * caller that only has an id and needs a handle. Doesn't verify the
   * sandbox exists; the first call fails with 404 if it doesn't. */
  static attach(id: string, options: SandboxOptions = {}): Sandbox {
    return new Sandbox(id, resolveClient(options));
  }

  /** A sandbox can drop out of this list via the daemon's own
   * `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`, not just an explicit `stop()`/
   * `snapshot()`. Use `listSnapshots({ sourceSandboxId })` to find what a
   * disappeared id turned into. */
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

  /** Resolves a name to a live sandbox (a round-trip, unlike `attach`).
   * Only resolves *live* — a name held by a stopped/snapshotted sandbox
   * gets a 409, not a silent resume; use `getOrCreate` for that. */
  static async byName(name: string, options: SandboxOptions = {}): Promise<Sandbox> {
    const client = resolveClient(options);
    const body = await request<SandboxByNameResponseBody>({
      ...client,
      method: "GET",
      path: `/sandboxes/by-name/${encodeURIComponent(name)}`,
    });
    return new Sandbox(body.id, client);
  }

  /** Returns a live sandbox by name as-is, resumes a stopped one, or
   * creates fresh if neither exists. `tags`/`vcpuCount`/`memSizeMib` only
   * apply to the create-fresh case. Race-safe server-side: two concurrent
   * calls for a brand-new name can't both create one. */
  static async getOrCreate(options: GetOrCreateSandboxOptions): Promise<{ sandbox: Sandbox; created: boolean }> {
    const client = resolveClient(options);
    const requestBody: GetOrCreateSandboxRequestBody = { name: options.name };
    if (options.tags !== undefined) requestBody.tags = options.tags;
    if (options.vcpuCount !== undefined) requestBody.vcpu_count = options.vcpuCount;
    if (options.memSizeMib !== undefined) requestBody.mem_size_mib = options.memSizeMib;
    const rateLimit = buildRateLimitRequestBody(options.rateLimit);
    if (rateLimit !== undefined) requestBody.rate_limit = rateLimit;
    const drives = buildDrivesRequestBody(options.drives);
    if (drives !== undefined) requestBody.drives = drives;
    if (options.env !== undefined) requestBody.env = options.env;
    const egress = buildEgressRequestBody(options.egress);
    if (egress !== undefined) requestBody.egress = egress;

    const body = await request<GetOrCreateSandboxResponseBody>({
      ...client,
      method: "POST",
      path: "/sandboxes/get-or-create",
      body: requestBody,
    });
    return { sandbox: new Sandbox(body.id, client), created: body.created };
  }

  async runCommand(command: string, args: string[] = [], options: RunCommandOptions = {}): Promise<ExecResult> {
    const requestBody: ExecRequestBody = { command, args };
    if (options.env !== undefined) requestBody.env = options.env;
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

  /** `mode` is raw permission bits (e.g. `0o644`), the same shape POSIX
   * `chmod(2)` takes — not a symbolic string like the `chmod` shell
   * command accepts. */
  async chmod(path: string, mode: number): Promise<void> {
    const requestBody: ChmodRequestBody = { path, mode };
    await request<void>({ ...this.client, method: "POST", path: `/sandboxes/${encodeURIComponent(this.id)}/chmod`, body: requestBody });
  }

  async chown(path: string, uid: number, gid: number): Promise<void> {
    const requestBody: ChownRequestBody = { path, uid, gid };
    await request<void>({ ...this.client, method: "POST", path: `/sandboxes/${encodeURIComponent(this.id)}/chown`, body: requestBody });
  }

  /** `parents: true` behaves like `mkdir -p` (creates missing parent
   * directories, succeeds if the target already exists); omitted/`false`
   * behaves like plain `mkdir` — fails if the parent is missing or the
   * target already exists. */
  async mkdir(path: string, options: { parents?: boolean } = {}): Promise<void> {
    const requestBody: MkdirRequestBody = { path, parents: options.parents };
    await request<void>({ ...this.client, method: "POST", path: `/sandboxes/${encodeURIComponent(this.id)}/mkdir`, body: requestBody });
  }

  async rename(from: string, to: string): Promise<void> {
    const requestBody: RenameRequestBody = { from, to };
    await request<void>({ ...this.client, method: "POST", path: `/sandboxes/${encodeURIComponent(this.id)}/rename`, body: requestBody });
  }

  /** A full byte-for-byte copy to a new path — `from` is left untouched,
   * unlike `rename`. */
  async copy(from: string, to: string): Promise<void> {
    const requestBody: CopyRequestBody = { from, to };
    await request<void>({ ...this.client, method: "POST", path: `/sandboxes/${encodeURIComponent(this.id)}/copy`, body: requestBody });
  }

  async symlink(target: string, linkPath: string): Promise<void> {
    const requestBody: SymlinkRequestBody = { target, link_path: linkPath };
    await request<void>({ ...this.client, method: "POST", path: `/sandboxes/${encodeURIComponent(this.id)}/symlink`, body: requestBody });
  }

  /** The target a symlink points at, exactly as stored — not
   * resolved/canonicalized. */
  async readlink(path: string): Promise<string> {
    const requestBody: ReadlinkRequestBody = { path };
    const body = await request<ReadlinkResponseBody>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/readlink`,
      body: requestBody,
    });
    return body.target;
  }

  async truncate(path: string, size: number): Promise<void> {
    const requestBody: TruncateRequestBody = { path, size };
    await request<void>({ ...this.client, method: "POST", path: `/sandboxes/${encodeURIComponent(this.id)}/truncate`, body: requestBody });
  }

  async listDir(path: string): Promise<DirEntry[]> {
    const requestBody: ListDirRequestBody = { path };
    const body = await request<ListDirResponseBody>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/list-dir`,
      body: requestBody,
    });
    return body.entries.map((e) => ({
      name: e.name,
      isDir: e.is_dir,
      isSymlink: e.is_symlink,
      size: e.size,
      mode: e.mode,
      mtime: new Date(e.mtime_unix * 1000),
    }));
  }

  /** Opens a live interactive PTY session — a real shell, not
   * `runCommand`'s one-shot request/response. Returns the raw
   * `WebSocket`: what you send becomes stdin, every message received is
   * interleaved stdout+stderr (see `examples/interactive-terminal`).
   * Needs a global `WebSocket` (any browser, or Node.js >= 22) — the one
   * method on this class that does, since a bidirectional stream needs a
   * different transport than `fetch`. `cols`/`rows` size the terminal
   * once at session start — no live resize yet (`ROADMAP.md`'s "Dev
   * servers and live preview"). Auth token goes as `?token=`, same
   * reason as `previewUrl()` — neither runtime's `WebSocket` can set
   * custom headers. */
  pty(options: PtyOptions = {}): WebSocket {
    if (typeof WebSocket === "undefined") {
      throw new Error(
        "Sandbox.pty() requires a runtime with a global WebSocket implementation (Node.js >= 22, or any browser) — this runtime doesn't have one.",
      );
    }
    const cols = options.cols ?? 80;
    const rows = options.rows ?? 24;
    const wsBaseUrl = this.client.baseUrl.replace(/^http/, "ws");
    const url = new URL(`${wsBaseUrl}/sandboxes/${encodeURIComponent(this.id)}/pty`);
    url.searchParams.set("cols", String(cols));
    url.searchParams.set("rows", String(rows));
    if (this.client.authToken !== undefined) {
      url.searchParams.set("token", this.client.authToken);
    }
    return new WebSocket(url);
  }

  /** Opens a local tunnel: code running inside the sandbox connects to
   * `guestPort` and reaches whatever's listening on `options.localPort`
   * on *this* machine — the reverse of `previewUrl()`. Node-only (see
   * `tunnel.ts`'s own doc comment for why `pty()`/`attachLogs()` don't
   * have this restriction and this does). Returns a handle; call
   * `.close()` when done forwarding. */
  async tunnel(guestPort: number, options: TunnelOptions): Promise<TunnelHandle> {
    return openTunnel(this.client, this.id, guestPort, options);
  }

  /** Starts `command` detached and returns immediately with a session
   * id — unlike `runCommand()`, doesn't wait for it to finish. Output is
   * captured from the moment it starts regardless of whether anything's
   * attached; call `attachLogs(sessionId)` any time for the full history
   * plus a live tail. Backs `kiln logs`/`kiln sandbox exec-stream`. Not
   * carried across `resume()`/`fork()`/restart. */
  async execStream(command: string, args: string[] = [], options: RunCommandOptions = {}): Promise<string> {
    const requestBody: StartExecStreamRequestBody = { command, args };
    if (options.env !== undefined) requestBody.env = options.env;
    const body = await request<StartExecStreamResponseBody>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/exec-stream`,
      body: requestBody,
    });
    return body.id;
  }

  /** Lists every streamed background exec session currently tracked
   * against this sandbox (see `execStream()`), whether still running or
   * already finished. */
  async listExecStreams(): Promise<ExecStreamSession[]> {
    const body = await request<ListExecStreamsResponseBody>({
      ...this.client,
      method: "GET",
      path: `/sandboxes/${encodeURIComponent(this.id)}/exec-stream`,
    });
    return body.sessions.map((s) => ({
      id: s.id,
      command: s.command,
      args: s.args,
      startedAt: new Date(s.started_at_unix * 1000),
      exitCode: s.exit_code,
    }));
  }

  /** Attaches to an `execStream()` session: replays everything captured
   * so far, then live-tails new output, whether or not the process has
   * finished. The daemon buffers, not this connection, so reconnecting
   * later replays the same full history. Same runtime requirement as
   * `pty()`. */
  attachLogs(sessionId: string): WebSocket {
    if (typeof WebSocket === "undefined") {
      throw new Error(
        "Sandbox.attachLogs() requires a runtime with a global WebSocket implementation (Node.js >= 22, or any browser) — this runtime doesn't have one.",
      );
    }
    const wsBaseUrl = this.client.baseUrl.replace(/^http/, "ws");
    const url = new URL(`${wsBaseUrl}/sandboxes/${encodeURIComponent(this.id)}/exec-stream/${encodeURIComponent(sessionId)}/logs`);
    if (this.client.authToken !== undefined) {
      url.searchParams.set("token", this.client.authToken);
    }
    return new WebSocket(url);
  }

  /** Mounts an S3-compatible bucket at `options.mountPath` via `rclone
   * mount` — ordinary `readFile()`/`writeFile()` there read/write
   * through. Needs the FUSE kernel + rclone rootfs setup in
   * `SELF_HOSTING.md`; raises `SandkilnApiError` on a guest-side mount
   * failure. Not carried across `resume()`/`fork()` — it's a live guest
   * FUSE process, already captured by Firecracker's own snapshot. */
  async mount(options: MountOptions): Promise<MountInfo> {
    const requestBody: CreateMountRequestBody = {
      bucket: options.bucket,
      endpoint: options.endpoint,
      access_key: options.accessKey,
      secret_key: options.secretKey,
      mount_path: options.mountPath,
    };
    if (options.readOnly !== undefined) requestBody.read_only = options.readOnly;
    const body = await request<MountResponseBody>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/mounts`,
      body: requestBody,
    });
    return mountInfoFromBody(body);
  }

  /** Lists every remote-storage mount currently active in this sandbox. */
  async listMounts(): Promise<MountInfo[]> {
    const body = await request<ListMountsResponseBody>({
      ...this.client,
      method: "GET",
      path: `/sandboxes/${encodeURIComponent(this.id)}/mounts`,
    });
    return body.mounts.map(mountInfoFromBody);
  }

  /** Tears down one mount (see `mount()`'s `id`). Best-effort — a failed
   * guest-side `umount` is a daemon-log warning, not a failed request. */
  async unmount(mountId: string): Promise<void> {
    await request<void>({
      ...this.client,
      method: "DELETE",
      path: `/sandboxes/${encodeURIComponent(this.id)}/mounts/${encodeURIComponent(mountId)}`,
    });
  }

  /** Stops this sandbox. Preserves state by default (snapshot, then
   * stop) — resume with `Sandbox.resume(snapshotId)` or
   * `getOrCreate({ name })`. Pass `{ keep: false }` to just destroy it
   * (e.g. a short-lived CI run). */
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

  /** URL a browser can open to reach a server listening on `port` inside
   * this sandbox, proxied via `/sandboxes/:id/preview/:port`. Pure and
   * network-free, like `attach` — the daemon proxies lazily per request.
   * Auth token appends as `?token=`, not a header, since a browser
   * tab/`<iframe>` can't set `Authorization` (daemon accepts either). */
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

  /** Saves full state to disk and stops this sandbox, returning a
   * snapshot id — `resume`/`fork` boot from it again. The daemon can also
   * do this on its own via `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS`. */
  async snapshot(): Promise<string> {
    const body = await request<SnapshotSandboxResponseBody>({
      ...this.client,
      method: "POST",
      path: `/sandboxes/${encodeURIComponent(this.id)}/snapshot`,
    });
    return body.snapshot_id;
  }

  /** Boots from a snapshot, consuming it — gone afterward, new sandbox
   * owns its state outright like a fresh `create`. Use `fork` to boot
   * from the same snapshot more than once. */
  static async resume(snapshotId: string, options: SandboxOptions = {}): Promise<Sandbox> {
    const client = resolveClient(options);
    const body = await request<ResumeSnapshotResponseBody>({
      ...client,
      method: "POST",
      path: `/snapshots/${encodeURIComponent(snapshotId)}/resume`,
    });
    return new Sandbox(body.id, client);
  }

  /** Boots from a snapshot *without* consuming it — parallel branches off
   * one prepared environment. Only one live fork per snapshot at a time
   * (it reopens the same rootfs/tap/IP the snapshot recorded); a second
   * `fork()` while one's still running rejects with 409. */
  static async fork(snapshotId: string, options: SandboxOptions = {}): Promise<Sandbox> {
    const client = resolveClient(options);
    const body = await request<ForkSnapshotResponseBody>({
      ...client,
      method: "POST",
      path: `/snapshots/${encodeURIComponent(snapshotId)}/fork`,
    });
    return new Sandbox(body.id, client);
  }

  /** Lists snapshots. `sourceSandboxId` narrows to the one snapshot a
   * given sandbox id became; omit to list every snapshot. */
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

/** `undefined` when the caller didn't attach any drives — matches every
 * other optional-array-field convention in this SDK (`undefined`, not
 * `[]`, means "the caller didn't ask for this"). */
function buildDrivesRequestBody(drives: DriveAttachmentOptions[] | undefined): DriveAttachmentRequestBody[] | undefined {
  if (drives === undefined) return undefined;
  return drives.map((d) => ({ id: d.id, read_only: d.readOnly }));
}

function mountInfoFromBody(body: MountResponseBody): MountInfo {
  return { id: body.id, bucket: body.bucket, endpoint: body.endpoint, mountPath: body.mount_path, readOnly: body.read_only };
}

/** Only includes the sub-fields the caller actually set — mirrors the
 * daemon's own per-field `#[serde(default)]` optionality on
 * `RateLimitRequest` rather than always sending both keys. */
function buildRateLimitRequestBody(rateLimit: RateLimitOptions | undefined): RateLimitRequestBody | undefined {
  if (rateLimit === undefined) return undefined;
  const body: RateLimitRequestBody = {};
  if (rateLimit.bandwidthBytesPerSec !== undefined) body.bandwidth_bytes_per_sec = rateLimit.bandwidthBytesPerSec;
  if (rateLimit.opsPerSec !== undefined) body.ops_per_sec = rateLimit.opsPerSec;
  return body;
}

function buildEgressRequestBody(egress: EgressPolicyOptions | undefined): EgressPolicyRequestBody | undefined {
  if (egress === undefined) return undefined;
  const body: EgressPolicyRequestBody = { mode: egress.mode };
  if (egress.allowCidrs !== undefined) body.allow_cidrs = egress.allowCidrs;
  if (egress.denyCidrs !== undefined) body.deny_cidrs = egress.denyCidrs;
  return body;
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
  if (options.imageId !== undefined) body.image_id = options.imageId;
  const rateLimit = buildRateLimitRequestBody(options.rateLimit);
  if (rateLimit !== undefined) body.rate_limit = rateLimit;
  const drives = buildDrivesRequestBody(options.drives);
  if (drives !== undefined) body.drives = drives;
  if (options.env !== undefined) body.env = options.env;
  const egress = buildEgressRequestBody(options.egress);
  if (egress !== undefined) body.egress = egress;
  return Object.keys(body).length > 0 ? body : undefined;
}

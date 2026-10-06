export interface SandboxOptions {
  baseUrl?: string;
  authToken?: string;
}

/** Caps host I/O via Firecracker's token-bucket rate limiter, applied to
 * the rootfs, every attached drive, and the network interface. At least
 * one field must be set and non-zero if present at all — the daemon
 * rejects an empty/all-zero `rateLimit` with 400. */
export interface RateLimitOptions {
  bandwidthBytesPerSec?: number;
  opsPerSec?: number;
}

/** Outbound network policy — one iptables chain per sandbox, `denyCidrs`
 * always winning on overlap (deny rules precede allow, first match
 * wins). `"deny_all"` blocks everything except `allowCidrs`; `"allow_all"`
 * (default) allows everything except `denyCidrs`. DNS to the bridge's
 * own gateway IP is structurally exempt either way. Survives
 * `snapshot()`/`resume()`/`fork()`. */
export interface EgressPolicyOptions {
  mode: "allow_all" | "deny_all";
  allowCidrs?: string[];
  denyCidrs?: string[];
}

export interface CreateSandboxOptions extends SandboxOptions {
  /** Unique among live sandboxes and held snapshots when claimed — 409
   * if taken. Optional. See `Sandbox.byName`/`getOrCreate`. */
  name?: string;
  tags?: Record<string, string>;
  /** Overrides the daemon default vCPU count. Rejected if `0` or past
   * `SANDKILN_MAX_VCPU_COUNT`. */
  vcpuCount?: number;
  /** Overrides the daemon default memory (MiB). Same ceiling semantics
   * as `vcpuCount`, checked against `SANDKILN_MAX_MEM_SIZE_MIB`. */
  memSizeMib?: number;
  /** Boots from a registered image (`Image.register`) instead of the
   * default rootfs. Rejected if the id isn't registered. */
  imageId?: string;
  /** Unlimited host I/O when omitted. See `RateLimitOptions`. */
  rateLimit?: RateLimitOptions;
  /** Persistent drives (`Drive.create`) to attach at boot. Read-write
   * (default) needs exclusive access — 409 if already attached anywhere;
   * `readOnly: true` may coexist with other read-only attachments, but
   * still conflicts with an existing read-write one. */
  drives?: DriveAttachmentOptions[];
  /** Baked in for this sandbox's whole lifetime — the base layer under
   * any `env` passed to `runCommand()`/`execStream()` (those win on
   * conflict). Persists through `snapshot()`/`resume()`/`fork()` like
   * `tags`. */
  env?: Record<string, string>;
  /** Omitted = unrestricted outbound via the shared bridge's catch-all
   * rule. See `EgressPolicyOptions`. */
  egress?: EgressPolicyOptions;
}

export interface DriveAttachmentOptions {
  id: string;
  readOnly?: boolean;
}

/** `tags`/`vcpuCount`/`memSizeMib`/`rateLimit`/`egress` apply only when
 * `getOrCreate` creates fresh — resuming an existing snapshot ignores
 * them, using what was recorded at snapshot time, like `resume`. */
export interface GetOrCreateSandboxOptions extends SandboxOptions {
  name: string;
  tags?: Record<string, string>;
  vcpuCount?: number;
  memSizeMib?: number;
  rateLimit?: RateLimitOptions;
  drives?: DriveAttachmentOptions[];
  env?: Record<string, string>;
  egress?: EgressPolicyOptions;
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

export interface RateLimitRequestBody {
  bandwidth_bytes_per_sec?: number;
  ops_per_sec?: number;
}

export interface DriveAttachmentRequestBody {
  id: string;
  read_only?: boolean;
}

export interface EgressPolicyRequestBody {
  mode: "allow_all" | "deny_all";
  allow_cidrs?: string[];
  deny_cidrs?: string[];
}

export interface CreateSandboxRequestBody {
  name?: string;
  tags?: Record<string, string>;
  vcpu_count?: number;
  mem_size_mib?: number;
  image_id?: string;
  rate_limit?: RateLimitRequestBody;
  drives?: DriveAttachmentRequestBody[];
  env?: Record<string, string>;
  egress?: EgressPolicyRequestBody;
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
  env?: Record<string, string>;
}

/** Merged on top of the sandbox's create-time `env` (wins on conflict),
 * not a replacement for it. */
export interface RunCommandOptions {
  env?: Record<string, string>;
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

export interface ChmodRequestBody {
  path: string;
  mode: number;
}

export interface ChownRequestBody {
  path: string;
  uid: number;
  gid: number;
}

export interface MkdirRequestBody {
  path: string;
  parents?: boolean;
}

export interface RenameRequestBody {
  from: string;
  to: string;
}

export interface CopyRequestBody {
  from: string;
  to: string;
}

export interface SymlinkRequestBody {
  target: string;
  link_path: string;
}

export interface ReadlinkRequestBody {
  path: string;
}

export interface ReadlinkResponseBody {
  target: string;
}

export interface TruncateRequestBody {
  path: string;
  size: number;
}

export interface ListDirRequestBody {
  path: string;
}

/** Sizes the terminal once at session start — no live resize yet (see
 * `Sandbox.pty()`). Defaults to 80x24. */
export interface PtyOptions {
  cols?: number;
  rows?: number;
}

export interface DirEntryBody {
  name: string;
  is_dir: boolean;
  is_symlink: boolean;
  size: number;
  mode: number;
  mtime_unix: number;
}

export interface ListDirResponseBody {
  entries: DirEntryBody[];
}

/** One `Sandbox.listDir()` entry — `mode` is permission bits
 * (`Sandbox.chmod()`'s shape), so it round-trips directly. */
export interface DirEntry {
  name: string;
  isDir: boolean;
  isSymlink: boolean;
  size: number;
  mode: number;
  mtime: Date;
}

export interface StartExecStreamRequestBody {
  command: string;
  args?: string[];
  env?: Record<string, string>;
}

export interface StartExecStreamResponseBody {
  id: string;
}

export interface ExecStreamSummaryBody {
  id: string;
  command: string;
  args: string[];
  started_at_unix: number;
  exit_code: number | null;
}

export interface ListExecStreamsResponseBody {
  sessions: ExecStreamSummaryBody[];
}

/** One background exec session (`Sandbox.listExecStreams()`/`execStream()`).
 * `exitCode` is `null` while still running. */
export interface ExecStreamSession {
  id: string;
  command: string;
  args: string[];
  startedAt: Date;
  exitCode: number | null;
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

export interface ListSnapshotsOptions extends SandboxOptions {
  /** Only the (at most one) snapshot taken from this sandbox id — a
   * sandbox id is retired the moment it's snapshotted. */
  sourceSandboxId?: string;
}

export interface SnapshotSummaryBody {
  id: string;
  source_sandbox_id: string;
  created_at_unix: number;
  tags: Record<string, string>;
  forked_into: string | null;
  name?: string | null;
}

export interface ListSnapshotsResponseBody {
  snapshots: SnapshotSummaryBody[];
}

export interface SnapshotInfo {
  id: string;
  sourceSandboxId: string;
  createdAt: Date;
  tags: Record<string, string>;
  /** Live sandbox currently forked from this snapshot, if any — while
   * set, `fork()`/`resume()` on this id both reject with 409. */
  forkedInto: string | null;
  /** Carried over from the sandbox this was taken from, if any. */
  name?: string | null;
}

/** Present only when the stop reported something (`keep=true`, default)
 * — `?keep=false` returns 204/no body, decoded as `undefined`. */
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
  rate_limit?: RateLimitRequestBody;
  drives?: DriveAttachmentRequestBody[];
  env?: Record<string, string>;
  egress?: EgressPolicyRequestBody;
}

export interface GetOrCreateSandboxResponseBody {
  id: string;
  created: boolean;
}

export interface ImageOptions {
  baseUrl?: string;
  authToken?: string;
}

/** A registered rootfs image (`Image.register`). Plain data, not a
 * handle like `Sandbox` — an image's only operation is delete, which
 * just needs an id. */
export interface ImageInfo {
  id: string;
  sizeMib: number;
  createdAt: Date;
  /** What holds this image (sandbox id, snapshot id, in-flight boot), or
   * `null`. Can't be deleted while set. */
  inUseBy: string | null;
  /** Always `false` — the daemon can't loop-mount to verify the guest
   * agent is baked in. See `verificationHint`. */
  guestAgentVerified: boolean;
  /** Guidance for verifying the guest agent out of band. */
  verificationHint: string;
}

export interface CreateImageRequestBody {
  id: string;
  path: string;
}

export interface ImageSummaryBody {
  id: string;
  size_mib: number;
  created_at_unix: number;
  in_use_by: string | null;
  guest_agent_verified: boolean;
  verification_hint: string;
}

export interface ListImagesResponseBody {
  images: ImageSummaryBody[];
}

export interface DriveOptions {
  baseUrl?: string;
  authToken?: string;
}

export interface CreateDriveRequestBody {
  size_mib: number;
}

export interface DriveHolderSummaryBody {
  holder: string;
  read_only: boolean;
}

export interface DriveSummaryBody {
  id: string;
  size_mib: number;
  created_at_unix: number;
  attached_to: DriveHolderSummaryBody[];
}

export interface ListDrivesResponseBody {
  drives: DriveSummaryBody[];
}

/** `"sandbox <id>"` or `"snapshot <id>"`. More than one means read-only
 * sharing; a drive can't be deleted while this is non-empty. */
export interface DriveHolder {
  holder: string;
  readOnly: boolean;
}

export interface DriveInfo {
  id: string;
  sizeMib: number;
  createdAt: Date;
  attachedTo: DriveHolder[];
}

export interface PoolOptions {
  baseUrl?: string;
  authToken?: string;
}

export interface CreatePoolRequestBody {
  id: string;
  image_id?: string;
  vcpu_count?: number;
  mem_size_mib?: number;
  warm_count: number;
  max_count?: number;
}

export interface PoolSummaryBody {
  id: string;
  image_id: string | null;
  vcpu_count: number;
  mem_size_mib: number;
  warm_count: number;
  max_count: number | null;
  warm_ready: number;
  claimed: number;
}

export interface ListPoolsResponseBody {
  pools: PoolSummaryBody[];
}

/** A configured pre-warmed pool (`Pool`). `vcpuCount`/`memSizeMib` are
 * already resolved to concrete values, unlike `CreatePoolOptions`'s
 * `undefined`-means-default shape. */
export interface PoolInfo {
  id: string;
  imageId: string | null;
  vcpuCount: number;
  memSizeMib: number;
  warmCount: number;
  /** Max combined warm+claimed instances — `null` means unbounded. */
  maxCount: number | null;
  /** Resumable snapshots actually sitting warm right now — can lag
   * `warmCount` right after creation or a claim; replenishment is
   * background, not instant. */
  warmReady: number;
  /** Live instances of this pool's profile right now (warm or
   * cold-created) — meaningful relative to `maxCount`. */
  claimed: number;
}

/** Mounts an S3-compatible bucket via `rclone mount` (`Sandbox.mount()`).
 * `endpoint` is always explicit, never defaulted to a provider.
 * `accessKey`/`secretKey` go into a `0600` rclone config file in the
 * guest, never a command-line arg. `mountPath` is created if missing. */
export interface MountOptions {
  bucket: string;
  endpoint: string;
  accessKey: string;
  secretKey: string;
  mountPath: string;
  readOnly?: boolean;
}

/** One active remote-storage mount, as returned by `Sandbox.mount()`/
 * `Sandbox.listMounts()`. Never carries credentials — those exist only
 * as a `0600` file inside the guest, not in any daemon-side record. */
export interface MountInfo {
  id: string;
  bucket: string;
  endpoint: string;
  mountPath: string;
  readOnly: boolean;
}

export interface CreateMountRequestBody {
  bucket: string;
  endpoint: string;
  access_key: string;
  secret_key: string;
  mount_path: string;
  read_only?: boolean;
}

export interface MountResponseBody {
  id: string;
  bucket: string;
  endpoint: string;
  mount_path: string;
  read_only: boolean;
}

export interface ListMountsResponseBody {
  mounts: MountResponseBody[];
}

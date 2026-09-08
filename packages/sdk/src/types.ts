export interface SandboxOptions {
  baseUrl?: string;
  authToken?: string;
}

/** Caps host I/O for a sandbox via Firecracker's own token-bucket rate
 * limiter, applied to the rootfs drive, every attached drive, and both
 * directions of the network interface. At least one of the two fields
 * must be set (and non-zero) if this is present at all — the daemon
 * rejects an empty or all-zero `rateLimit` with a 400, the same
 * "reject, don't silently no-op" convention as `vcpuCount`/`memSizeMib`. */
export interface RateLimitOptions {
  bandwidthBytesPerSec?: number;
  opsPerSec?: number;
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
  /** Boots from a registered image (see `Image.register`) instead of the
   * daemon's configured default rootfs. Omitted means today's behavior,
   * unchanged. Rejected if no image with this id is currently
   * registered. */
  imageId?: string;
  /** Unlimited host I/O (the default) when omitted. See `RateLimitOptions`. */
  rateLimit?: RateLimitOptions;
  /** Existing persistent drives (see `Drive.create`) to attach at boot,
   * each becoming its own block device inside the guest. A read-write
   * attachment (the default, `readOnly` omitted or `false`) needs
   * exclusive access — rejected with a 409 if the drive is already
   * attached anywhere; a `readOnly: true` attachment may coexist with
   * any number of other read-only attachments of the same drive, but
   * still conflicts with an existing read-write one. Omitted means no
   * drives, unchanged from before this existed. */
  drives?: DriveAttachmentOptions[];
}

export interface DriveAttachmentOptions {
  id: string;
  readOnly?: boolean;
}

/** `tags`/`vcpuCount`/`memSizeMib`/`rateLimit` are used only when
 * `Sandbox.getOrCreate` actually creates a fresh sandbox — resuming an
 * existing snapshot under this name ignores them, using what was
 * recorded on it when it was taken, same as `Sandbox.resume`. */
export interface GetOrCreateSandboxOptions extends SandboxOptions {
  name: string;
  tags?: Record<string, string>;
  vcpuCount?: number;
  memSizeMib?: number;
  rateLimit?: RateLimitOptions;
  drives?: DriveAttachmentOptions[];
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

export interface CreateSandboxRequestBody {
  name?: string;
  tags?: Record<string, string>;
  vcpu_count?: number;
  mem_size_mib?: number;
  image_id?: string;
  rate_limit?: RateLimitRequestBody;
  drives?: DriveAttachmentRequestBody[];
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

/** One entry from `Sandbox.listDir()` — `mode` is permission bits only
 * (e.g. `0o644`), the same shape `Sandbox.chmod()` takes, so an entry's
 * `mode` can be handed straight back to `chmod()`. */
export interface DirEntry {
  name: string;
  isDir: boolean;
  isSymlink: boolean;
  size: number;
  mode: number;
  mtime: Date;
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
  /** Only snapshots taken from this original sandbox id — the mechanism
   * for finding out whether a sandbox id you had turned into a snapshot
   * (via `Sandbox.snapshot()` or the daemon's auto-suspend), and what its
   * new snapshot id is. At most one snapshot can ever match, since a
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
  /** Id of the live sandbox currently forked from this snapshot, if any —
   * see `Sandbox.fork()`. While set, `Sandbox.fork()`/`Sandbox.resume()`
   * on this snapshot id both reject with a 409. */
  forkedInto: string | null;
  /** Carried over from the sandbox this was taken from, if any — see
   * `CreateSandboxOptions.name`. */
  name?: string | null;
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
  rate_limit?: RateLimitRequestBody;
  drives?: DriveAttachmentRequestBody[];
}

export interface GetOrCreateSandboxResponseBody {
  id: string;
  created: boolean;
}

export interface ImageOptions {
  baseUrl?: string;
  authToken?: string;
}

/** A registered rootfs image a sandbox can boot from — see
 * `Image.register`. Not a class/handle like `Sandbox`: an image has no
 * instance operations besides delete, which only ever needs an id, so
 * this is plain data, matching `SandboxInfo`'s shape for `Sandbox.list`. */
export interface ImageInfo {
  id: string;
  sizeMib: number;
  createdAt: Date;
  /** What currently holds this image, if anything (a sandbox id, a
   * snapshot id, or a sandbox still being created from it) — `null` if
   * nothing does. An image can't be deleted while this is set. */
  inUseBy: string | null;
  /** Always `false` — the daemon cannot verify the guest agent is baked
   * into a registered image (that needs loop-mounting it as root, which
   * the daemon deliberately doesn't have). See `verificationHint`. */
  guestAgentVerified: boolean;
  /** Guidance for verifying the guest agent out of band before relying on
   * this image — the daemon can never fill in `guestAgentVerified: true`
   * itself. */
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

/** Who currently holds a drive — `"sandbox <id>"` or `"snapshot <id>"`.
 * More than one holder means it's attached read-only to several at
 * once; a drive can't be deleted while this is non-empty. */
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

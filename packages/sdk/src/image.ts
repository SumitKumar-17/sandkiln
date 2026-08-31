import { resolveClient } from "./client.js";
import { request } from "./http.js";
import type {
  CreateImageRequestBody,
  ImageInfo,
  ImageOptions,
  ImageSummaryBody,
  ListImagesResponseBody,
} from "./types.js";

/**
 * Registered rootfs images: named, daemon-tracked ext4 files a sandbox can
 * boot from instead of the daemon's configured default rootfs (see
 * `CreateSandboxOptions.imageId`). Unlike `Sandbox`, this is a namespace of
 * static operations rather than a stateful handle class — an image has no
 * instance behavior besides delete, and delete only ever needs an id.
 *
 * Registration does not upload a file: `path` must already exist on the
 * host the daemon process itself runs on. Converting a Docker/OCI image
 * into a bootable rootfs is a separate, larger problem this SDK (and the
 * daemon) doesn't attempt — see `images/README.md` in the sandkiln
 * repository.
 */
export class Image {
  /**
   * Registers an already-built ext4 rootfs file at `path` (a path on the
   * daemon's own host filesystem, not uploaded) under `id` — a stable,
   * memorable name reusable across many `Sandbox.create({ imageId })`
   * calls.
   *
   * The daemon cannot verify the guest agent is baked into the image: that
   * would need loop-mounting the file as root, which the daemon
   * deliberately doesn't have (see `ImageInfo.guestAgentVerified`'s doc
   * comment). Check `verificationHint` on the result, or run
   * `scripts/preflight-check.sh --root-checks --rootfs-image <path>`
   * against the file out of band before relying on it — the single most
   * common way a custom image otherwise fails is booting fine but never
   * responding to `runCommand`.
   */
  static async register(id: string, path: string, options: ImageOptions = {}): Promise<ImageInfo> {
    const client = resolveClient(options);
    const requestBody: CreateImageRequestBody = { id, path };
    const body = await request<ImageSummaryBody>({
      ...client,
      method: "POST",
      path: "/images",
      body: requestBody,
    });
    return toImageInfo(body);
  }

  static async list(options: ImageOptions = {}): Promise<ImageInfo[]> {
    const client = resolveClient(options);
    const body = await request<ListImagesResponseBody>({
      ...client,
      method: "GET",
      path: "/images",
    });
    return body.images.map(toImageInfo);
  }

  /**
   * Permanently removes an image's registration and backing file.
   * Rejected (`409`) while any live sandbox, in-flight boot, or held
   * snapshot still references it.
   */
  static async delete(id: string, options: ImageOptions = {}): Promise<void> {
    const client = resolveClient(options);
    await request<void>({
      ...client,
      method: "DELETE",
      path: `/images/${encodeURIComponent(id)}`,
    });
  }
}

function toImageInfo(body: ImageSummaryBody): ImageInfo {
  return {
    id: body.id,
    sizeMib: body.size_mib,
    createdAt: new Date(body.created_at_unix * 1000),
    inUseBy: body.in_use_by,
    guestAgentVerified: body.guest_agent_verified,
    verificationHint: body.verification_hint,
  };
}

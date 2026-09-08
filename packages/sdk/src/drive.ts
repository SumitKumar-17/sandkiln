import { resolveClient } from "./client.js";
import { request } from "./http.js";
import type {
  CreateDriveRequestBody,
  DriveInfo,
  DriveOptions,
  DriveSummaryBody,
  ListDrivesResponseBody,
} from "./types.js";

/**
 * Persistent drives: attachable filesystem storage that outlives any
 * single sandbox and can be reattached to a new one — for state that
 * should survive well past any one VM's lifetime. Attach one at create
 * time via `CreateSandboxOptions.drives`. Like `Image`, this is a
 * namespace of static operations rather than a stateful handle class —
 * a drive has no instance behavior besides delete.
 *
 * A drive has no "detach" call of its own: detaching happens implicitly
 * when the sandbox holding it is stopped (`Sandbox.stop`), which drops
 * the attachment without touching the drive's backing file. `Drive.delete`
 * permanently destroys a drive and is rejected (409) while anything
 * still holds it.
 */
export class Drive {
  /** Creates a new empty drive of `sizeMib` MiB, ready to attach to a
   * sandbox at create time. */
  static async create(sizeMib: number, options: DriveOptions = {}): Promise<DriveInfo> {
    const client = resolveClient(options);
    const requestBody: CreateDriveRequestBody = { size_mib: sizeMib };
    const body = await request<DriveSummaryBody>({
      ...client,
      method: "POST",
      path: "/drives",
      body: requestBody,
    });
    return toDriveInfo(body);
  }

  static async list(options: DriveOptions = {}): Promise<DriveInfo[]> {
    const client = resolveClient(options);
    const body = await request<ListDrivesResponseBody>({
      ...client,
      method: "GET",
      path: "/drives",
    });
    return body.drives.map(toDriveInfo);
  }

  /** Permanently removes a drive and its backing file. Rejected (`409`)
   * while any sandbox or held snapshot still attaches it — see
   * `DriveInfo.attachedTo`. */
  static async delete(id: string, options: DriveOptions = {}): Promise<void> {
    const client = resolveClient(options);
    await request<void>({
      ...client,
      method: "DELETE",
      path: `/drives/${encodeURIComponent(id)}`,
    });
  }
}

function toDriveInfo(body: DriveSummaryBody): DriveInfo {
  return {
    id: body.id,
    sizeMib: body.size_mib,
    createdAt: new Date(body.created_at_unix * 1000),
    attachedTo: body.attached_to.map((h) => ({ holder: h.holder, readOnly: h.read_only })),
  };
}

import { Sandbox } from "sandkiln";

const MOUNT_PATH = "/mnt/bucket";

const endpoint = requireEnv("S3_ENDPOINT");
const accessKey = requireEnv("S3_ACCESS_KEY");
const secretKey = requireEnv("S3_SECRET_KEY");
const bucket = requireEnv("S3_BUCKET");

const daemonUrl = (process.env.SANDKILN_DAEMON_URL ?? "http://127.0.0.1:7777").replace(/\/+$/, "");
const authToken = process.env.SANDKILN_AUTH_TOKEN;

function requireEnv(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`${name} is not set. This example needs an S3-compatible endpoint you provide yourself -- see README.md.`);
    process.exit(2);
  }
  return value;
}

// Mounts have no SDK surface yet, only the daemon's HTTP API -- see
// README.md. Everything else here goes through the published SDK.
async function daemon(method, path, body) {
  const res = await fetch(`${daemonUrl}${path}`, {
    method,
    headers: { "content-type": "application/json", ...(authToken ? { authorization: `Bearer ${authToken}` } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${method} ${path} -> ${res.status} ${await res.text()}`);
  return res.status === 204 ? null : await res.json();
}

async function main() {
  console.log("Creating sandbox...");
  const sandbox = await Sandbox.create({ tags: { example: "remote-storage-mount" } });
  console.log(`Sandbox ${sandbox.id} ready.`);

  let mountId = null;
  try {
    console.log(`Mounting bucket "${bucket}" from ${endpoint} at ${MOUNT_PATH}...`);
    const mount = await daemon("POST", `/sandboxes/${sandbox.id}/mounts`, {
      bucket,
      endpoint,
      access_key: accessKey,
      secret_key: secretKey,
      mount_path: MOUNT_PATH,
      read_only: false,
    });
    mountId = mount.id;
    console.log(`Mounted as ${mountId}.`);

    const { mounts } = await daemon("GET", `/sandboxes/${sandbox.id}/mounts`);
    console.log(`Sandbox reports ${mounts.length} mount(s): ${mounts.map((m) => `${m.bucket} -> ${m.mount_path}`).join(", ")}\n`);

    const objectName = `sandkiln-example-${Date.now()}.txt`;
    const contents = `written from inside sandbox ${sandbox.id} at ${new Date().toISOString()}\n`;
    console.log(`Writing ${objectName} through the mount...`);
    await sandbox.writeFile(`${MOUNT_PATH}/${objectName}`, contents);

    const listing = await sandbox.runCommand("ls", ["-l", MOUNT_PATH]);
    console.log(`Contents of ${MOUNT_PATH} inside the guest:\n${listing.stdout.trimEnd()}\n`);

    const readBack = new TextDecoder().decode(await sandbox.readFile(`${MOUNT_PATH}/${objectName}`));
    console.log(readBack === contents ? `Read it back byte-for-byte through the mount.` : `Read back different content: ${JSON.stringify(readBack)}`);
    console.log(`${objectName} is now a real object in "${bucket}" -- it outlives this sandbox.\n`);

    console.log("Unmounting...");
    await daemon("DELETE", `/sandboxes/${sandbox.id}/mounts/${mountId}`);
    mountId = null;

    // `mountpoint` exits non-zero for a path that isn't a mount point --
    // proof the FUSE process is actually gone from the guest, not just
    // dropped from the daemon's own listing.
    const check = await sandbox.runCommand("mountpoint", ["-q", MOUNT_PATH]);
    console.log(check.exitCode === 0 ? `${MOUNT_PATH} still reports as mounted.` : `${MOUNT_PATH} is no longer a mount point.`);
  } finally {
    if (mountId !== null) {
      await daemon("DELETE", `/sandboxes/${sandbox.id}/mounts/${mountId}`).catch(() => {});
    }
    await sandbox.stop({ keep: false });
    console.log(`\nSandbox ${sandbox.id} stopped.`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});

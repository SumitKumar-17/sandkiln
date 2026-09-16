import { Sandbox } from "sandkiln";

const MOUNT_PATH = "/mnt/bucket";

const endpoint = requireEnv("S3_ENDPOINT");
const accessKey = requireEnv("S3_ACCESS_KEY");
const secretKey = requireEnv("S3_SECRET_KEY");
const bucket = requireEnv("S3_BUCKET");

function requireEnv(name) {
  const value = process.env[name];
  if (!value) {
    console.error(`${name} is not set. This example needs an S3-compatible endpoint you provide yourself -- see README.md.`);
    process.exit(2);
  }
  return value;
}

async function main() {
  console.log("Creating sandbox...");
  const sandbox = await Sandbox.create({ tags: { example: "remote-storage-mount" } });
  console.log(`Sandbox ${sandbox.id} ready.`);

  let mountId = null;
  try {
    console.log(`Mounting bucket "${bucket}" from ${endpoint} at ${MOUNT_PATH}...`);
    const mount = await sandbox.mount({ bucket, endpoint, accessKey, secretKey, mountPath: MOUNT_PATH });
    mountId = mount.id;
    console.log(`Mounted as ${mountId}.`);

    const mounts = await sandbox.listMounts();
    console.log(`Sandbox reports ${mounts.length} mount(s): ${mounts.map((m) => `${m.bucket} -> ${m.mountPath}`).join(", ")}\n`);

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
    await sandbox.unmount(mountId);
    mountId = null;

    // `mountpoint` exits non-zero for a path that isn't a mount point --
    // proof the FUSE process is actually gone from the guest, not just
    // dropped from the daemon's own listing.
    const check = await sandbox.runCommand("mountpoint", ["-q", MOUNT_PATH]);
    console.log(check.exitCode === 0 ? `${MOUNT_PATH} still reports as mounted.` : `${MOUNT_PATH} is no longer a mount point.`);
  } finally {
    if (mountId !== null) {
      await sandbox.unmount(mountId).catch(() => {});
    }
    await sandbox.stop({ keep: false });
    console.log(`\nSandbox ${sandbox.id} stopped.`);
  }
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});

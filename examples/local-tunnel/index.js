import { createServer } from "node:http";
import { Sandbox } from "sandkiln";

const LOCAL_PORT = 9999;
const GUEST_PORT = 8080;
const SECRET = `hello from the real local machine, ${Date.now()}`;

// Stands in for a real local service -- a dev server, a database, an
// internal API -- running on *this* machine, not inside any sandbox.
const localServer = createServer((_req, res) => res.end(SECRET));

async function main() {
  await new Promise((resolve) => localServer.listen(LOCAL_PORT, "127.0.0.1", resolve));
  console.log(`Local server listening on 127.0.0.1:${LOCAL_PORT}.`);

  console.log("Creating sandbox...");
  const sandbox = await Sandbox.create({ tags: { example: "local-tunnel" } });
  console.log(`Sandbox ${sandbox.id} ready.`);

  try {
    console.log(`Opening tunnel: sandbox:${GUEST_PORT} -> this machine's 127.0.0.1:${LOCAL_PORT}...`);
    const tunnel = await sandbox.tunnel(GUEST_PORT, { localPort: LOCAL_PORT });
    console.log(`Tunnel ${tunnel.tunnelId} open.`);

    // The guest-side listener binds asynchronously (StartTunnel returns
    // once the daemon has told the guest agent to start, not once it's
    // actually bound) -- a brief wait here, same as any "is the other
    // side listening yet" race elsewhere in this project.
    await new Promise((resolve) => setTimeout(resolve, 300));

    console.log(`Running curl inside the sandbox against http://127.0.0.1:${GUEST_PORT}/ (which only exists because of the tunnel)...`);
    const result = await sandbox.runCommand("curl", ["-s", "-m", "5", `http://127.0.0.1:${GUEST_PORT}/`]);
    console.log(`Sandbox received: ${JSON.stringify(result.stdout)}`);

    if (result.stdout !== SECRET) {
      throw new Error(`expected the sandbox to receive exactly what the local server sent -- got ${JSON.stringify(result.stdout)}`);
    }
    console.log("Match -- the sandbox really did reach a service running on this machine, not inside any VM.");

    await tunnel.close();
    console.log("Tunnel closed.");
  } finally {
    await sandbox.stop({ keep: false });
    localServer.close();
    console.log("Sandbox destroyed, local server closed.");
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});

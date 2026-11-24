# sandkiln

TypeScript/JavaScript client for [sandkiln](https://github.com/SumitKumar-17/sandkiln),
which runs untrusted or AI-generated code in hardware-isolated Firecracker
microVMs. It talks to a `sandkilnd` daemon you run yourself; there is no hosted
service (see [self-hosting](https://sandkiln.mintlify.site/getting-started/self-hosting)).

## Install

```
npm install sandkiln
```

## Usage

```ts
import { Sandbox } from "sandkiln";

const sandbox = await Sandbox.create({ tags: { env: "ci" } });

const result = await sandbox.runCommand("python3", ["analyze.py"]);
console.log(result.stdout, result.exitCode);

await sandbox.writeFile("/tmp/config.json", JSON.stringify({ ok: true }));
const bytes = await sandbox.readFile("/tmp/config.json");

await sandbox.stop(); // keeps a resumable snapshot by default; { keep: false } destroys
```

Also available: filesystem operations, `pty()`, `previewUrl()`, `tunnel()`,
`execStream()`, snapshots (`snapshot`, `Sandbox.resume`, `Sandbox.fork`), named
sandboxes (`getOrCreate`), remote storage mounts, and the `Drive`, `Image` and
`Pool` namespaces.

## Configuration

- Daemon URL: `baseUrl` option or `SANDKILN_DAEMON_URL` (default `http://127.0.0.1:7777`).
- Auth: `authToken` option or `SANDKILN_AUTH_TOKEN`. Omit for an unauthenticated local daemon.

## Docs

Full method reference: [JS/TS SDK docs](https://sandkiln.mintlify.site/api-reference/js-sdk/overview).
Daemon features without a wrapper here (snapshot lineage, time-travel restore)
are tracked in the [roadmap](https://github.com/SumitKumar-17/sandkiln/blob/main/ROADMAP.md).

## License

MIT

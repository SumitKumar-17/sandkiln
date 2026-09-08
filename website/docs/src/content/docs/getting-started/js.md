---
title: "First sandbox: JS/TS"
description: Boot and run your first sandbox with the JS/TS SDK.
---

```bash
npm install sandkiln
```

```ts
import { Sandbox } from "sandkiln";

const sandbox = await Sandbox.create({ tags: { env: "ci" } });
const result = await sandbox.runCommand("python3", ["analyze.py"]);
console.log(result.stdout, result.exitCode);
await sandbox.stop(); // preserved as a resumable snapshot by default
```

`Sandbox.create()`, `list()`, and every instance method reuse the `baseUrl`/`authToken` you pass in, or fall back to the `SANDKILN_DAEMON_URL`/`SANDKILN_AUTH_TOKEN` environment variables — see [Auth](/docs/concepts/auth/). Full method reference: [JS/TS SDK](/docs/reference/js-sdk/).

## Next

- Give the sandbox a name so you can find it again later: [Named sandboxes & persistent stop](/docs/concepts/named-sandboxes/).
- Boot from your own image instead of the daemon's default: [Boot from a custom image](/docs/guides/custom-image/).

---
title: "First sandbox: JS/TS"
description: Boot and run your first sandbox with the JS/TS SDK.
---

```bash
npm install sandkiln
```

```ts
import { Sandbox } from "sandkiln";

const sandbox = await Sandbox.create({ tags: { env: "demo" } });
console.log("created:", sandbox.id);
// created: 1fa417db-10af-4e77-97a0-8d26347f13e2

const result = await sandbox.runCommand("python3", ["-c", "print(21 * 2)"]);
console.log(result.stdout.trim(), result.exitCode);
// 42 0

await sandbox.stop(); // preserved as a resumable snapshot by default
```

`Sandbox.create()`, `list()`, and every instance method reuse the `baseUrl`/`authToken` you pass in, or fall back to the `SANDKILN_DAEMON_URL`/`SANDKILN_AUTH_TOKEN` environment variables — see [Auth](../../concepts/auth/).

## A failed command doesn't reject the promise

`runCommand` reports whatever the command actually did — exit code and `stderr` included — it doesn't throw just because the exit code was non-zero. Check `result.exitCode` the same way a shell script checks `$?`:

```ts
const failed = await sandbox.runCommand("cat", ["/does/not/exist"]);
console.log("exit code:", failed.exitCode, "stderr:", failed.stderr.trim());
// exit code: 1 stderr: cat: /does/not/exist: No such file or directory
```

`SandkilnApiError` is reserved for the request itself failing — a bad sandbox id, an unreachable daemon, a validation error. Calling any method after `stop()` (without `resume()`-ing it first) is the everyday way to see one:

```ts
import { SandkilnApiError } from "sandkiln";

try {
  await sandbox.runCommand("echo", ["hi"]);
} catch (err) {
  if (err instanceof SandkilnApiError) {
    console.log(err.message);
    // sandbox not found: 1fa417db-10af-4e77-97a0-8d26347f13e2
  }
}
```

## Next

- Give the sandbox a name so you can find it again later: [Named sandboxes & persistent stop](../../concepts/named-sandboxes/).
- Boot from your own image instead of the daemon's default: [Boot from a custom image](../../guides/custom-image/).
- Full method reference: [JS/TS SDK](../../reference/js-sdk/).

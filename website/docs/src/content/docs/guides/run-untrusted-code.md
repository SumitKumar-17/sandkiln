---
title: Run untrusted AI-generated code safely
description: The default, no-configuration-needed path for executing code you don't trust.
---

This is the case sandkiln is built for: code you didn't write — an AI agent's output, a user upload, a third-party script — needs to run somewhere that a bug or a deliberately hostile payload in it can't reach your own systems.

## The default is already isolated

You don't need to configure anything extra for the baseline isolation guarantee — every sandbox is a real Firecracker microVM with its own kernel, its own filesystem, its own network namespace, the moment you call `Sandbox.create()`. See [Privilege model](/docs/architecture/privilege-model/) for what that boundary actually is and isn't.

```ts
import { Sandbox } from "sandkiln";

const sandbox = await Sandbox.create({ tags: { purpose: "untrusted-exec" } });
const result = await sandbox.runCommand("python3", ["-c", untrustedCode]);
// result.stdout / result.stderr / result.exitCode — inspect before trusting
await sandbox.stop({ keep: false }); // no reason to keep state around for a one-shot run
```

## Things worth doing on top of the default

- **Set a resource ceiling.** `vcpu_count`/`mem_size_mib` on create, checked against `SANDKILN_MAX_VCPU_COUNT`/`SANDKILN_MAX_MEM_SIZE_MIB` — stops one sandbox from starving the host.
- **Tag it.** `tags: { purpose: "untrusted-exec", ... }` makes it easy to filter and audit later via `GET /sandboxes?tag.purpose=untrusted-exec`.
- **Destroy, don't preserve, one-shot runs.** `stop({ keep: false })` (or `?keep=false` / `kiln sandbox rm --destroy`) skips the snapshot entirely for a sandbox you never intend to resume — no reason to pay the pause-and-snapshot cost or leave state sitting on disk.
- **For genuinely adversarial workloads, turn on the jailer.** `SANDKILN_JAILER_ENABLED` adds chroot, cgroup v2 resource limits, and a dedicated unprivileged uid per VM — see [Privilege model](/docs/architecture/privilege-model/). It's opt-in and not yet proven against a real installed jailer binary on hardware, so verify it in your own environment before relying on it for something genuinely hostile.
- **Set an idle timeout.** `SANDKILN_IDLE_TIMEOUT_SECS` or `SANDKILN_AUTO_SUSPEND_TIMEOUT_SECS` on the daemon reclaims a sandbox a caller forgot to stop — see [Auto-suspend idle sandboxes](/docs/guides/auto-suspend/).

# sandkiln

A compute primitive for safely running untrusted or AI-generated code.

sandkiln boots hardware-isolated Firecracker microVMs on demand, gives you a
programmatic API to execute commands and read/write files inside them, and
tears them down when you're done. Each sandbox is a real microVM — its own
kernel, its own filesystem, its own network namespace, isolated from every
other sandbox on the same host. Built for the same shape of problem as AI
agent sandboxes, code playgrounds, and untrusted-code execution services:
isolate first, then run.

**Website**: https://sumitkumar-17.github.io/sandkiln/ (architecture, real
benchmark numbers, live feature status)

## Quickstart

There's no hosted service — you run a `sandkilnd` daemon yourself. See
[SELF_HOSTING.md](SELF_HOSTING.md) for the full setup (KVM, Firecracker,
the base rootfs image, tap device pool, daemon config), then talk to it
from a client.

```
npm install sandkiln
```

```ts
import { Sandbox } from "sandkiln";

const sandbox = await Sandbox.create({ tags: { env: "ci" } });
const result = await sandbox.runCommand("python3", ["analyze.py"]);
console.log(result.stdout, result.exitCode);
await sandbox.stop();
```

Python and a CLI (`kiln`) ship the same operations — see
[`packages/python`](packages/python) and [`packages/cli`](packages/cli).

## Status

Active development. The core primitive, networking, auth, tags, file ops,
and all three clients (JS/TS, Python, CLI) work and are verified against
real hardware — see [CHANGELOG.md](CHANGELOG.md) for what shipped and
[ROADMAP.md](ROADMAP.md) for what's still open (snapshots/persistence,
managed images, drives, streamed output). The plan is a direction, not a
spec, and keeps changing as the project gets built.

Picking this up as a contributor (human or agent)? Read
[AGENTS.md](AGENTS.md) first — it covers non-obvious things this project
already hit and fixed once.

## Architecture

- **`core/`** — Rust workspace: `sandkiln-protocol` (the wire format
  shared by host and guest), `sandkiln-guest-agent` (a static binary that
  runs inside each microVM), `sandkiln-vmm` (drives Firecracker and
  networking), `sandkiln-daemon` (the HTTP API, `sandkilnd`).
- **`packages/sdk`** — [`sandkiln`](https://www.npmjs.com/package/sandkiln)
  on npm, the JS/TS client.
- **`packages/python`** — `sandkiln` on PyPI (not yet published), the
  Python client, mirroring the JS SDK exactly.
- **`packages/cli`** — `kiln`, the command-line interface.
- **`images/`** — kernel and rootfs build scripts for sandbox base images.
- **`scripts/`** — dev-box setup: tap pool, network bridge, DNS proxy,
  remote sync.
- **`website/`** — the project site, deployed via GitHub Pages.

## License

MIT

# sandkiln

A compute primitive for safely running untrusted or AI-generated code.

sandkiln boots hardware-isolated Firecracker microVMs on demand, gives you a
programmatic API to execute commands, read/write files, and stream logs
inside them, and tears them down (or snapshots them) when you're done. It's
built for the same shape of problem as AI agent sandboxes, code playgrounds,
and untrusted-code execution services: isolate first, then run.

## Why

Running code you didn't write — AI agent output, user uploads, third-party
scripts — next to your own systems is a bad idea. sandkiln gives each
execution its own microVM: its own kernel, its own filesystem, its own
network namespace. A compromised sandbox cannot see or touch anything
outside itself.

## Status

Early, active development. Not published yet. See [ROADMAP.md](ROADMAP.md)
for where this is headed — the plan is a direction, not a spec, and will
keep changing as the project gets built.

## Architecture (target shape)

- **`core/`** — Rust workspace: VM lifecycle management on top of
  Firecracker, a guest agent that runs inside each microVM, and a daemon
  that exposes it all over HTTP.
- **`packages/sdk`** — `sandkiln`, the JS/TS client SDK.
- **`packages/cli`** — `kiln`, the command-line interface.
- **`images/`** — kernel and rootfs build scripts for sandbox base images.

## License

MIT

<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/logo-dark.svg">
    <img alt="sandkiln" src="docs/images/logo-light.svg" height="56">
  </picture>
</h1>

[![Rust CI](https://github.com/SumitKumar-17/sandkiln/actions/workflows/ci-rust.yml/badge.svg)](https://github.com/SumitKumar-17/sandkiln/actions/workflows/ci-rust.yml)
[![npm: sandkiln](https://img.shields.io/npm/v/sandkiln?label=npm%3A%20sandkiln)](https://www.npmjs.com/package/sandkiln)
[![PyPI: sandkiln](https://img.shields.io/pypi/v/sandkiln?label=PyPI%3A%20sandkiln)](https://pypi.org/project/sandkiln/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Sandboxes for agents: run untrusted or AI-generated code in hardware-isolated Firecracker microVMs, on your own hardware.**

<p align="center">
  <a href="https://sandkiln.vercel.app">Website</a> ·
  <a href="https://sandkiln.mintlify.site">Docs</a> ·
  <a href="https://sandkiln.mintlify.site/architecture/overview">Architecture</a> ·
  <a href="https://sandkiln.mintlify.site/engineering/29-what-a-start-latency-number-means">Performance</a> ·
  <a href="CHANGELOG.md">Changelog</a>
</p>

Every sandbox is a real microVM with its own kernel, filesystem and network interface. You run the `sandkilnd` daemon yourself on a Linux host with KVM. A sandbox answers its first command in milliseconds from the default warm pool. You can snapshot, fork and time-travel it, give it an egress policy, attach persistent drives, and use it from TypeScript, Python, a CLI, an MCP server or a GitHub Action.

## Quick start

Needs a Linux x86_64 host with `/dev/kvm`, `sudo` for one-time setup, and Rust ([rustup](https://rustup.rs)).

```bash
git clone https://github.com/SumitKumar-17/sandkiln.git && cd sandkiln
scripts/setup.sh                 # build, Firecracker, test image, tap pool, DNS forwarder, config
scripts/sandkilnd-ctl.sh start   # start the daemon and wait for /healthz
npm install -g sandkiln-cli && kiln sandbox create
```

Set an `auth_token` before listening beyond localhost. Full walk-through: [self-hosting quickstart](https://sandkiln.mintlify.site/getting-started/self-hosting).

## Speed

Run it yourself: `scripts/dev.sh bench-suite` measures create, first command, resume, fork and snapshot against your daemon and writes a diffable result. Recorded runs, method and caveats: [docs/benchmarks.md](docs/benchmarks.md).

## Where things live

There are two public sites and this repository.

| | What it is | Where |
| --- | --- | --- |
| **Website** | The product page: what it is, install, a real session | [sandkiln.vercel.app](https://sandkiln.vercel.app) |
| **Docs** | Concepts, guides, self-hosting | [Introduction](https://sandkiln.mintlify.site/getting-started/introduction) |
| | API reference: HTTP, JS/TS, Python, CLI, MCP | [Reference](https://sandkiln.mintlify.site/api-reference/http-api/overview) |
| | Architecture: crates, request path, one page per mechanism | [Architecture](https://sandkiln.mintlify.site/architecture/overview) |
| | Performance: how speed is measured, every optimisation | [Performance](https://sandkiln.mintlify.site/engineering/29-what-a-start-latency-number-means) |
| | Case studies: bugs, root causes, fixes | [Case studies](https://sandkiln.mintlify.site/engineering/27-correctness-snapshot-disk-corruption) |
| **Repository** | Decisions, findings, benchmarks | [docs/](docs/README.md) |
| | What is done and planned, release history | [ROADMAP.md](ROADMAP.md), [CHANGELOG.md](CHANGELOG.md) |
| | Contributing, build and test | [docs/development.md](docs/development.md), [AGENTS.md](AGENTS.md) |

Install the clients with `npm install sandkiln`, `npm install -g sandkiln-cli` (the `kiln` command), `pip install sandkiln` or `npm install -g sandkiln-mcp`.

MIT licensed, see [LICENSE](LICENSE).

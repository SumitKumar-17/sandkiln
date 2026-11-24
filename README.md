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

Medians from the committed benchmark runs, one shared dev box (directional). The chart is generated from them.

![Measured latency, log scale](docs/images/benchmarks.svg)

Method, history and caveats: [docs/benchmarks.md](docs/benchmarks.md).

## Where to go

| I want to | Go to |
| --- | --- |
| See every capability | [Introduction](https://sandkiln.mintlify.site/getting-started/introduction) |
| Use it from JS/TS, Python or the CLI | [JS](https://sandkiln.mintlify.site/getting-started/js), [Python](https://sandkiln.mintlify.site/getting-started/python), [CLI](https://sandkiln.mintlify.site/getting-started/cli) (`npm i sandkiln`, `pip install sandkiln`) |
| Look up an endpoint or method | [API reference](https://sandkiln.mintlify.site/api-reference/http-api/overview) |
| Self-host and configure | [Self-hosting](https://sandkiln.mintlify.site/getting-started/self-hosting), [SELF_HOSTING.md](SELF_HOSTING.md) |
| Understand how it works | [Architecture](https://sandkiln.mintlify.site/architecture/overview), [engineering posts](https://sandkiln.mintlify.site/engineering/01-system-overview) |
| Read decisions and findings | [docs/](docs/README.md) |
| See what is done and planned | [ROADMAP.md](ROADMAP.md), [CHANGELOG.md](CHANGELOG.md) |
| Contribute | [docs/development.md](docs/development.md), [AGENTS.md](AGENTS.md) |

MIT licensed, see [LICENSE](LICENSE).

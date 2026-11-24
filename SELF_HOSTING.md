# Self-hosting sandkiln

There is no hosted service: you run `sandkilnd` on your own Linux x86_64 host with KVM. The documentation lives in the docs
site, in this repository under `docs-site/`, and online at https://sandkiln.mintlify.site/.

```bash
scripts/setup.sh                 # idempotent bootstrap
scripts/sandkilnd-ctl.sh start   # returns once /healthz answers
```

- [Self-hosting quickstart](docs-site/getting-started/self-hosting.mdx): requirements, setup, verify, config pointers, shutdown, reflink clones, troubleshooting
- [Manual host setup](docs-site/self-hosting/manual-setup.mdx): every step `setup.sh` automates, images, mounts, tap pool, capabilities, jailer
- [Configuration](docs-site/self-hosting/configuration.mdx): resolution order, `sandkilnd --check-config|--print-config|--config-reference`, every setting; example file [`sandkiln.example.toml`](sandkiln.example.toml)
- [Operations and security](docs-site/self-hosting/operations.mdx): control script, systemd, upgrades, troubleshooting, persistent state, security posture

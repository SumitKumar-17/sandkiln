---
title: Self-hosting quickstart
description: Get a sandkilnd daemon running from a bare Linux host.
---

There's no hosted sandkiln service — every instance is self-hosted. The full, tested path from a bare Linux host (with Rust and Firecracker's own prerequisites — KVM, sudo — already available) to a real daemon booting real microVMs is two commands:

```bash
scripts/setup.sh          # builds everything, fetches a test image,
                           # injects the guest agent, creates the tap
                           # pool, grants CAP_NET_ADMIN — idempotent,
                           # safe to re-run
scripts/sandkilnd-ctl.sh start
```

That's a real, working daemon end to end, but booting from the small Firecracker CI test image (~300MiB, missing `ca-certificates` and any language runtime). For a real base image with Node.js, Python, and common tooling (needs sudo and ~8GiB free disk; takes several minutes):

```bash
scripts/setup.sh --production
```

## What's actually going on

`setup.sh` and `sandkilnd-ctl.sh` automate what used to be a dozen manual steps with paths that had to match by hand — building the Rust workspace, fetching/building a kernel and rootfs, injecting the guest agent into it, creating a persistent tap device pool, and granting the daemon `CAP_NET_ADMIN` (see [Privilege model](../../architecture/privilege-model/) for why it's this specific capability and not root). If you need to customize something the flags don't cover, or a step fails and you want to understand exactly what it's doing, **[`SELF_HOSTING.md`](https://github.com/SumitKumar-17/sandkiln/blob/main/SELF_HOSTING.md) in the repository is the full, section-by-section guide** this quickstart is condensed from — requirements, manual build steps, networking internals, permissions, configuration reference, running as a persistent systemd service, upgrade/rebuild notes, and troubleshooting.

## Verify it worked

```bash
curl -X POST http://127.0.0.1:7777/sandboxes -d '{}'
# {"id": "..."}
```

Once you have an id back, move on to your language of choice: [JS/TS](../js/), [Python](../python/), or the [CLI](../cli/).

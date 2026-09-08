---
title: Boot from a custom image
description: Register a rootfs and boot sandboxes from it.
---

The daemon's default rootfs is whatever `SANDKILN_BASE_ROOTFS` is configured to. Registering your own image lets you boot from something else — your own tooling baked in, a pinned runtime version — without touching that daemon-wide default.

## Register it

```bash
curl -X POST http://127.0.0.1:7777/images \
  -d '{"id": "node-lts-custom", "path": "/home/t1000/images/node-lts-custom.ext4"}'
```

`path` is a path on the **daemon's own host filesystem** — this isn't a file upload. Build the image first (`images/build-universal-image.sh` is the reference build script for the project's own universal image, adaptable for a custom one) and get it onto the daemon's host before registering.

## Verify the guest agent is actually there

The response includes `"guest_agent_verified": false` — always, by design, because the unprivileged daemon can't loop-mount your image to check. Do this yourself before relying on it:

```bash
scripts/preflight-check.sh --root-checks --rootfs-image /home/t1000/images/node-lts-custom.ext4
```

## Boot from it

```ts
const sandbox = await Sandbox.create({ imageId: "node-lts-custom" });
```

A nonexistent `imageId` fails fast with a `404`, checked before the slow boot ever starts. See [Custom & managed images](../concepts/images/) for the full mechanics, including deletion and what's genuinely not done yet (OCI/Docker conversion).

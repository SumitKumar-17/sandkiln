# Authentication, images and docs

## Authentication

Done: a single bearer token (`SANDKILN_AUTH_TOKEN`) gates every
`/sandboxes*` route through daemon middleware; `/healthz` and `/metrics`
stay open. Off by default for local dev with a startup warning so that is
never silent. Compared in constant time (finding 15). A self-hosted
project, so a shared secret stands in for what a hosted platform would do
with OIDC.

Planned: per-token scoping (which sandboxes a token can see or act on) once
more than one caller shares a daemon.

## Base and custom images

- Universal base image: Ubuntu with current Node.js LTS, Python, common CLI
  tooling and full root in the guest; the default every sandbox boots from.
  Built reproducibly from `images/`.
- Planned: a small catalog of managed images for common runtimes.
- Done (partial) custom images:
  - `POST /images` registers an already-built ext4 rootfs from a host path
    into `SANDKILN_IMAGES_DIR` under a caller-given id.
  - `GET /images` lists them with `in_use_by`, an always-`false`
    `guest_agent_verified` and a `verification_hint`: the unprivileged
    daemon cannot loop-mount a candidate to confirm the agent is baked in,
    so run `scripts/preflight-check.sh --root-checks --rootfs-image <path>`
    out of band first.
  - `DELETE /images/:id` is refused (409) while a live sandbox, in-flight
    boot or held snapshot references it.
  - `POST /sandboxes` takes `image_id`, carried through snapshot, resume and
    fork like `name` and `tags`.
  - SDK and CLI: `Image.register/list/delete` in both SDKs, `imageId`/
    `image_id` on create, `kiln image ls|create|rm`, `kiln sandbox create
    --image`.
  - Not done: OCI conversion (still only a built ext4 file), and booting by
    image name through `get-or-create`.

## Documentation and examples

- Docs site (`docs-site/`): Getting Started, Core Concepts, Guides,
  Internals, Reference (HTTP API, JS/TS SDK, Python SDK, CLI) and
  Architecture, deployed independently of the marketing site (see
  `docs-site/AGENTS.md`). The marketing site (`website/`, hosted as static pages with a mirror) links
  out and no longer carries any docs content of its own. The docs site is
  deployed through its host's own GitHub integration at
  sandkiln.mintlify.site.
- Ten runnable example projects, one per feature surface: code playground,
  AI-agent runner, dev-server preview, interactive terminal, pre-warmed
  pool, streamed exec/logs, remote storage mount, snapshot lifecycle,
  named/persistent sandboxes, egress policy. See `examples/AGENTS.md`.

## Networking

Every sandbox leases a tap from a pre-created pool
(`scripts/host-setup/create-tap-pool.sh`) attached to a shared bridge with a
static IP. The daemon runs unprivileged with `CAP_NET_ADMIN` in its ambient
set (`scripts/host-setup/grant-net-admin.sh`). Verified: two concurrent
sandboxes, distinct IPs, both resolving DNS and reaching the internet.
Sandboxes are isolated from each other by Linux bridge port isolation (a tap
reaches the gateway and uplink but not another tap): cross-sandbox ping fails
while gateway ping and outbound HTTP work.
